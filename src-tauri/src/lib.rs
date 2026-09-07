use std::collections::HashSet;
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_notification::NotificationExt;

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
    tray: Mutex<Option<tauri::tray::TrayIcon>>,
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
    win.navigate(url.parse::<tauri::Url>().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let _ = win.set_title("Mellow");
    let _ = win.show();
    let _ = win.set_focus();
    Ok(url)
}

#[tauri::command]
fn forget_server(state: State<'_, AppState>, url: String) {
    let mut s = state.load();
    s.servers.retain(|x| x != &url);
    if s.last.as_deref() == Some(url.as_str()) {
        s.last = None;
    }
    state.save(&s);
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
fn set_unread(app: tauri::AppHandle, state: State<'_, AppState>, count: String) {
    if let Ok(tray) = state.tray.lock() {
        if let Some(t) = tray.as_ref() {
            let _ = t.set_tooltip(Some(if count.is_empty() {
                "Mellow".into()
            } else {
                format!("({count}) Mellow")
            }));
        }
    }
    let _ = app;
}

/* ─────────────────────────────── boot ───────────────────────────────── */

fn show_and_focus(win: &WebviewWindow) {
    let _ = win.show();
    let _ = win.unminimize();
    let _ = win.set_focus();
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                show_and_focus(&win);
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir().expect("config dir");
            app.manage(AppState {
                settings_path: config_dir.join("settings.json"),
                launcher_url: Mutex::new(None),
                tray: Mutex::new(None),
            });

            // Single window, created here so we can attach the native bridge:
            // - initialization_script injects the "change server" rail button on
            //   server pages without needing remote IPC (custom commands are
            //   ACL-blocked on remote origins);
            // - on_navigation catches mellow-desktop:// from that button.
            let nav_handle = handle.clone();
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Mellow")
                .inner_size(1180.0, 760.0)
                .min_inner_size(880.0, 580.0)
                .center()
                .resizable(true)
                .additional_browser_args("--ignore-certificate-errors --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection")
                .initialization_script(include_str!("desktop-bridge.js"))
                .on_navigation(move |url| {
                    if url.scheme() == "mellow-desktop" {
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
                    true
                })
                .build()?;

            if let Some(icon) = app.default_window_icon() {
                let _ = win.set_icon(icon.clone());
            }

            let launcher = win.url().ok().map(|u| u.to_string());
            if let Some(ref l) = launcher {
                if !l.starts_with("about:") {
                    *app.state::<AppState>().launcher_url.lock().unwrap() = launcher;
                }
            }

            // Tray with menu
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
            let win = app.get_webview_window("main").expect("main window");
            let win_clone = win.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = win_clone.hide();
                }
            });

            // If a server is saved and reachable, navigate straight to it.
            // If unreachable, stay on the bundled launcher (index.html) which already handles reachability reporting.
            let settings = app.state::<AppState>().load();
            if let Some(last) = settings.last.clone() {
                if tcp_reachable(&last, Duration::from_millis(900)) {
                    if let Ok(url) = last.parse::<tauri::Url>() {
                        let _ = win.navigate(url);
                    }
                }
            }

            // Sync unread count (encoded in document.title by the web app)
            // into the tray tooltip. Works purely via injected JS; harmless
            // if __TAURI__ is unavailable on the remote origin.
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
