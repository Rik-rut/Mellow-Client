pub mod proxy;

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri::{Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;

mod local_env {
    /// Proxy the webview origin through http://127.0.0.1 on mac/linux
    /// (WKWebView cannot bypass a self-signed cert); MELLOW_PROXY=1 forces it
    /// on Windows too, to test the exact mac/linux code path locally.
    pub fn use_proxy() -> bool {
        if !cfg!(target_os = "windows") {
            return true;
        }
        matches!(std::env::var("MELLOW_PROXY").as_deref(), Ok("1"))
    }
}

const DEFAULT_PORT: u16 = 6767;
const DISCOVERY_PORT: u16 = 6768;
const DISCOVERY_PROBE: &[u8] = b"MELLOW_DISCOVER_V1";

/* ────────────────────────── settings on disk ────────────────────────── */

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    servers: Vec<String>,
    #[serde(default)]
    last: Option<String>,
}

struct AppState {
    settings_path: PathBuf,
    launcher_url: Mutex<Option<String>>,
    #[cfg(desktop)]
    tray: Mutex<Option<tauri::tray::TrayIcon>>,
    /// One local reverse proxy per distinct server url: server url -> local port.
    proxies: Mutex<HashMap<String, u16>>,
}

impl AppState {
    /// Navigate target for a server url: on mac/linux (or with MELLOW_PROXY=1)
    /// a per-server local reverse proxy serves the real https server over
    /// http://127.0.0.1:<stable port> so WebKit sees a secure context.
    fn proxied_url(&self, target: &str) -> Result<String, String> {
        if !local_env::use_proxy() {
            return Ok(target.to_string());
        }
        {
            let map = self.proxies.lock().map_err(|_| "state lock".to_string())?;
            if let Some(port) = map.get(target) {
                return Ok(format!("http://127.0.0.1:{port}"));
            }
        }
        let port = tauri::async_runtime::block_on(proxy::start_proxy(target))?;
        self.proxies
            .lock()
            .map_err(|_| "state lock".to_string())?
            .insert(target.to_string(), port);
        Ok(format!("http://127.0.0.1:{port}"))
    }

    fn is_proxy_port(&self, port: u16) -> bool {
        self.proxies
            .lock()
            .map(|m| m.values().any(|p| *p == port))
            .unwrap_or(false)
    }
}

impl AppState {
    fn load(&self) -> Settings {
        match std::fs::read_to_string(&self.settings_path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }
    fn save(&self, settings: &Settings) {
        if let Some(parent) = self.settings_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &self.settings_path,
            serde_json::to_string_pretty(settings).unwrap_or_default(),
        );
    }
}

/* ────────────────────────────── external urls ───────────────────────── */

fn open_external_url(app: &tauri::AppHandle, url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = app;
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "android")]
    {
        // No xdg-open on Android: the opener plugin posts an ACTION_VIEW intent
        // through the wry activity — zero hand-written JNI.
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().open_url(url, None::<String>);
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "ios")]
    {
        let _ = (app, url);
    }
}

/* ─────────────────── android foreground-service bridge ───────────────── */

/// Rust owns the keep-alive (FGS) lifecycle: it dispatches static-method calls
/// into `MellowBridge` on the Kotlin side via wry's main-thread JNI pipe —
/// no JS-invocable commands, so remote-origin ACL is irrelevant (plan §2.3).
#[cfg(target_os = "android")]
mod keepalive {
    use std::sync::atomic::{AtomicBool, Ordering};
    use wry::prelude::{dispatch, find_class};

    const BRIDGE_CLASS: &str = "dev/mellow/client/MellowBridge";
    static CONNECTED: AtomicBool = AtomicBool::new(false);

    pub fn set_connected(connected: bool) {
        // Only cross the JNI boundary on real transitions — the launcher
        // navigation at startup must not poke a service that never started.
        if CONNECTED.swap(connected, Ordering::SeqCst) == connected {
            return;
        }
        dispatch(move |env, activity, _webview| {
            let Ok(cls) = find_class(env, activity, BRIDGE_CLASS.into()) else {
                eprintln!("[mellow] MellowBridge class not found");
                return;
            };
            let arg = if connected { 1 } else { 0 };
            let _ = env.call_static_method(&cls, "setConnected", "(I)V", &[arg.into()]);
        });
    }

    pub fn set_badge(count: &str) {
        if !CONNECTED.load(Ordering::SeqCst) {
            return;
        }
        let count = count.to_string();
        dispatch(move |env, activity, _webview| {
            let Ok(cls) = find_class(env, activity, BRIDGE_CLASS.into()) else {
                return;
            };
            let Ok(s) = env.new_string(&count) else { return };
            let _ = env.call_static_method(&cls, "setBadge", "(Ljava/lang/String;)V", &[(&s).into()]);
        });
    }
}

/* ────────────────────────────── urls ────────────────────────────────── */

fn normalize_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Enter a server address, e.g. 192.168.1.5:6767".into());
    }
    let (scheme, rest) = if trimmed.starts_with("http://") {
        ("http", &trimmed[7..])
    } else if trimmed.starts_with("https://") {
        ("https", &trimmed[8..])
    } else {
        ("https", trimmed)
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.contains('/') || rest.contains(' ') {
        return Err("That does not look like a server address".into());
    }
    let host_port = match rest.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => rest.to_string(),
        Some(_) => return Err("Invalid port in address".into()),
        None => format!("{rest}:{DEFAULT_PORT}"),
    };
    Ok(format!("{scheme}://{host_port}"))
}

fn host_port_of(url: &str) -> Result<(String, u16), String> {
    let rest = url
        .split_once("://")
        .map(|x| x.1)
        .ok_or("Invalid url")?;
    let host = rest.split('/').next().unwrap_or(rest);
    if let Some((h, p)) = host.rsplit_once(':') {
        Ok((h.to_string(), p.parse().map_err(|_| "Invalid port")?))
    } else {
        Ok((host.to_string(), if url.starts_with("https") { 443 } else { 80 }))
    }
}

fn tcp_reachable(url: &str, timeout: Duration) -> bool {
    let (host, port) = match host_port_of(url) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let addrs: Vec<SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return false,
    };
    addrs
        .iter()
        .any(|a| TcpStream::connect_timeout(a, timeout).is_ok())
}

/* ───────────────────────────── commands ─────────────────────────────── */

#[derive(Serialize)]
struct ServerInfo {
    servers: Vec<String>,
    last: Option<String>,
    last_unreachable: bool,
}

#[tauri::command]
fn get_servers(app: tauri::AppHandle, state: State<'_, AppState>) -> ServerInfo {
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(url) = win.url() {
            let s = url.to_string();
            if !s.starts_with("about:") {
                if let Ok(mut lock) = state.launcher_url.lock() {
                    *lock = Some(s);
                }
            }
        }
    }
    let s = state.load();
    let last_unreachable = match &s.last {
        Some(u) => !tcp_reachable(u, Duration::from_millis(800)),
        None => false,
    };
    ServerInfo {
        servers: s.servers,
        last: s.last.clone(),
        last_unreachable,
    }
}

#[tauri::command]
fn test_server(url: String) -> Result<String, String> {
    let url = normalize_url(&url)?;
    if tcp_reachable(&url, Duration::from_secs(2)) {
        Ok(url)
    } else {
        Err(format!("Could not reach {url}"))
    }
}

#[tauri::command]
fn connect(app: tauri::AppHandle, state: State<'_, AppState>, url: String) -> Result<String, String> {
    let url = normalize_url(&url)?;
    if !tcp_reachable(&url, Duration::from_secs(2)) {
        return Err(format!("Could not reach {url}"));
    }
    let mut s = state.load();
    s.servers.retain(|x| x != &url);
    s.servers.push(url.clone());
    while s.servers.len() > 5 {
        s.servers.remove(0);
    }
    s.last = Some(url.clone());
    state.save(&s);

    let win = app.get_webview_window("main").ok_or("no main window")?;
    let nav_url = state.proxied_url(&url)?;
    win.navigate(nav_url.parse::<tauri::Url>().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let _ = win.set_title("Mellow");
    let _ = win.show();
    let _ = win.set_focus();
    Ok(url)
}

#[tauri::command]
#[allow(unused_variables)]
fn forget_server(app: tauri::AppHandle, state: State<'_, AppState>, url: String) {
    let mut s = state.load();
    let was_last = s.last.as_deref() == Some(url.as_str());
    s.servers.retain(|x| x != &url);
    if was_last {
        s.last = None;
    }
    state.save(&s);
    #[cfg(target_os = "android")]
    if was_last {
        keepalive::set_connected(false);
    }
}

#[tauri::command]
fn switch_server(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or("no main window")?;
    let launcher = state
        .launcher_url
        .lock()
        .map_err(|_| "state lock".to_string())?
        .clone()
        .unwrap_or_else(|| "http://tauri.localhost/index.html".to_string());
    win.navigate(launcher.parse::<tauri::Url>().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let _ = win.set_title("Mellow");
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

#[derive(Serialize)]
struct Discovered {
    url: String,
    name: String,
}

#[tauri::command]
fn scan_lan() -> Vec<Discovered> {
    let mut found: HashSet<String> = HashSet::new();
    let mut results: Vec<Discovered> = Vec::new();
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return results,
    };
    if socket.set_broadcast(true).is_err() {
        return results;
    }
    let _ = socket.set_nonblocking(true);
    let target = (std::net::Ipv4Addr::new(255, 255, 255, 255), DISCOVERY_PORT);
    let deadline = Instant::now() + Duration::from_millis(1400);
    let mut last_send = Instant::now() - Duration::from_secs(1);
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline {
        if last_send.elapsed() > Duration::from_millis(450) {
            let _ = socket.send_to(DISCOVERY_PROBE, target);
            last_send = Instant::now();
        }
        match socket.recv_from(&mut buf) {
            Ok((len, peer)) => {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&buf[..len]) {
                    if value.get("service").and_then(|v| v.as_str()) == Some("mellow") {
                        let port = value.get("port").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_PORT as u64);
                        let https = value.get("https").and_then(|v| v.as_bool()).unwrap_or(true);
                        let name = value.get("name").and_then(|v| v.as_str()).unwrap_or("Mellow").to_string();
                        let url = format!(
                            "{}://{}:{}",
                            if https { "https" } else { "http" },
                            peer.ip(),
                            port
                        );
                        if found.insert(url.clone()) {
                            results.push(Discovered { url, name });
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(40)),
            Err(_) => break,
        }
    }
    results
}

#[tauri::command]
fn desktop_notify(app: tauri::AppHandle, title: String, body: String) {
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

#[tauri::command]
#[allow(unused_variables)]
fn set_unread(app: tauri::AppHandle, state: State<'_, AppState>, count: String) {
    #[cfg(desktop)]
    {
        if let Ok(tray) = state.tray.lock() {
            if let Some(t) = tray.as_ref() {
                let _ = t.set_tooltip(Some(if count.is_empty() {
                    "Mellow".into()
                } else {
                    format!("({count}) Mellow")
                }));
            }
        }
    }
    // Android: the same "N+ Mellow" title count drives the ongoing
    // foreground-service notification body via the Kotlin bridge.
    #[cfg(target_os = "android")]
    keepalive::set_badge(&count);
}

/* ─────────────────────────────── boot ───────────────────────────────── */

#[cfg(desktop)]
fn show_and_focus(win: &WebviewWindow) {
    let _ = win.show();
    #[cfg(desktop)]
    let _ = win.unminimize();
    let _ = win.set_focus();
}

/// Single window, created here so we can attach the native bridge:
/// - initialization_script injects the "change server" rail button on
///   server pages without needing remote IPC (custom commands are
///   ACL-blocked on remote origins);
/// - on_navigation catches mellow-desktop:// from that button.
fn build_main_window(
    app: &mut tauri::App,
) -> Result<WebviewWindow, Box<dyn std::error::Error>> {
    let nav_handle = app.handle().clone();
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Mellow")
        .inner_size(1180.0, 760.0)
        .min_inner_size(880.0, 580.0)
        .resizable(true)
        .disable_drag_drop_handler();

    #[cfg(desktop)]
    let builder = builder.center();

    // WebView2-only flags: the method is ignored on mac/linux (tauri docs).
    #[cfg(target_os = "windows")]
    let builder = builder.additional_browser_args(
        "--ignore-certificate-errors \
         --autoplay-policy=no-user-gesture-required \
         --enable-gpu-rasterization \
         --enable-zero-copy \
         --ignore-gpu-blocklist \
         --enable-accelerated-video-decode \
         --enable-accelerated-video-encode \
         --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion,IntensiveWakeUpThrottling",
    );

    let win = builder
        .initialization_script(include_str!("desktop-bridge.js"))
        .on_navigation(move |url| {
            if url.scheme() == "mellow-desktop" {
                if url.host_str() == Some("open-url") {
                    if let Some((_, target)) = url.query_pairs().find(|(k, _)| k == "url") {
                        open_external_url(&nav_handle, &target);
                    }
                    return false;
                }
                if url.host_str() == Some("switch-server") || url.path() == "/switch-server" {
                    let h = nav_handle.clone();
                    let hc = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        let Some(w) = hc.get_webview_window("main") else { return };
                        let base = hc
                            .state::<AppState>()
                            .launcher_url
                            .lock()
                            .ok()
                            .and_then(|u| u.clone())
                            .unwrap_or_else(|| "http://tauri.localhost/index.html".to_string());
                        let base = base.split('?').next().unwrap_or(&base).to_string();
                        if let Ok(u) = base.parse::<tauri::Url>() {
                            let _ = w.navigate(u);
                            let _ = w.set_title("Mellow");
                        }
                    });
                    return false;
                }
                return false;
            }
            if url.scheme() == "http" || url.scheme() == "https" {
                let is_launcher = url.host_str() == Some("tauri.localhost")
                    || url.host_str() == Some("localhost")
                    || url.scheme() == "tauri";
                let settings = nav_handle.state::<AppState>().load();
                let is_server = settings.servers.iter().chain(settings.last.iter()).any(|s| {
                    if let Ok(su) = s.parse::<tauri::Url>() {
                        su.host_str() == url.host_str() && su.port() == url.port()
                    } else {
                        false
                    }
                });
                // A proxied server origin (http://127.0.0.1:<port>) is the
                // server on mac/linux — must not be shunted to the browser.
                let is_proxied_server = matches!(url.host_str(), Some("127.0.0.1"))
                    && url.port().map(|p| nav_handle.state::<AppState>().is_proxy_port(p)).unwrap_or(false);
                // Android: the keep-alive foreground service mirrors the
                // connected state — on while a server page is loaded, off
                // back at the launcher (plan §2.3, lifecycle owned by Rust).
                #[cfg(target_os = "android")]
                {
                    if is_launcher {
                        keepalive::set_connected(false);
                    } else if is_server || is_proxied_server {
                        keepalive::set_connected(true);
                    }
                }
                if !is_launcher && !is_server && !is_proxied_server {
                    open_external_url(&nav_handle, url.as_str());
                    return false;
                }
            }
            true
        })
        .build()?;
    Ok(win)
}

/// Desktop-only chrome: tray icon with menu + close-button hides to tray.
#[cfg(desktop)]
fn setup_desktop(app: &tauri::App, win: &WebviewWindow) -> Result<(), Box<dyn std::error::Error>> {
    let show_i = MenuItem::with_id(app, "show", "Open Mellow", true, None::<&str>)?;
    let switch_i = MenuItem::with_id(app, "switch", "Switch server", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &switch_i, &quit_i])?;
    let tray = TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Mellow")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    show_and_focus(&w);
                }
            }
            "switch" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let url_str = state
                        .launcher_url
                        .lock()
                        .ok()
                        .and_then(|u| u.clone())
                        .unwrap_or_else(|| "http://tauri.localhost/index.html".to_string());
                    if let Some(w) = app.get_webview_window("main") {
                        if let Ok(u) = url_str.parse::<tauri::Url>() {
                            let _ = w.navigate(u);
                            let _ = w.set_title("Mellow");
                            show_and_focus(&w);
                        }
                    }
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(true) {
                         let _ = w.hide();
                    } else {
                        show_and_focus(&w);
                    }
                }
            }
        })
        .build(app)?;
    *app.state::<AppState>().tray.lock().unwrap() = Some(tray);

    // Close button hides to tray
    let win_clone = win.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = win_clone.hide();
        }
    });
    Ok(())
}

/// Android: permissions (POST_NOTIFICATIONS), the back-button minimize and the
/// keep-alive service itself live in the Kotlin project (gen/android); the
/// service start/stop is dispatched from on_navigation / forget_server above.
/// This hook exists so the platform split has one obvious entry point.
#[cfg(target_os = "android")]
fn setup_android(_app: &tauri::App, _win: &WebviewWindow) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_notification::init());

    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(win) = app.get_webview_window("main") {
            show_and_focus(&win);
        }
    }));

    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_opener::init());

    builder
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir().expect("config dir");
            app.manage(AppState {
                settings_path: config_dir.join("settings.json"),
                launcher_url: Mutex::new(None),
                #[cfg(desktop)]
                tray: Mutex::new(None),
                proxies: Mutex::new(HashMap::new()),
            });

            let win = build_main_window(app)?;

            #[cfg(desktop)]
            if let Some(icon) = app.default_window_icon() {
                let _ = win.set_icon(icon.clone());
            }

            let launcher = win.url().ok().map(|u| u.to_string());
            if let Some(ref l) = launcher {
                if !l.starts_with("about:") {
                    *app.state::<AppState>().launcher_url.lock().unwrap() = launcher;
                }
            }

            #[cfg(desktop)]
            setup_desktop(app, &win)?;
            #[cfg(target_os = "android")]
            setup_android(app, &win);

            // If a server is saved and reachable, navigate straight to it.
            // If unreachable, stay on the bundled launcher (index.html) which already handles reachability reporting.
            let settings = app.state::<AppState>().load();
            if let Some(last) = settings.last.clone() {
                if tcp_reachable(&last, Duration::from_millis(900)) {
                    let nav = app
                        .state::<AppState>()
                        .proxied_url(&last)
                        .unwrap_or_else(|e| {
                            eprintln!("[mellow] proxy start failed, navigating direct: {e}");
                            last.clone()
                        });
                    if let Ok(url) = nav.parse::<tauri::Url>() {
                        let _ = win.navigate(url);
                    }
                }
            }

            // Sync unread count (encoded in document.title by the web app)
            // into the tray tooltip (desktop) / ongoing notification badge
            // (android). Works purely via injected JS; harmless if __TAURI__
            // is unavailable on the remote origin.
            let timer_handle = handle.clone();
            thread::spawn(move || {
                let js = "if (window.__TAURI__ && window.__TAURI__.core) { \
                    var m = /^(\\d+\\+?)/.exec(document.title); \
                    window.__TAURI__.core.invoke('set_unread', { count: m ? m[1] : '' }).catch(function(){}); }";
                loop {
                    thread::sleep(Duration::from_secs(3));
                    if let Some(w) = timer_handle.get_webview_window("main") {
                        let _ = w.eval(js);
                    }
                }
            });

            let _ = handle;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_servers,
            test_server,
            connect,
            forget_server,
            switch_server,
            scan_lan,
            desktop_notify,
            set_unread
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mellow client");
}
