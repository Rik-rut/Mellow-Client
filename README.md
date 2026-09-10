# Mellow Desktop Client

Tauri v2 desktop client for self-hosted [Mellow](../local-chat) LAN chat servers.
Installers are built for Windows (NSIS), macOS (universal `.dmg`, unsigned) and
Linux (`.deb` + `.AppImage`) by GitHub Actions on every `v*` tag.

On macOS/Linux the client transparently reverse-proxies the server to
`http://127.0.0.1:<port>` (stable per server URL) so voice calls work in WebKit
without trusting the self-signed cert; on Windows the original direct-https path
is unchanged (`MELLOW_PROXY=1` forces the proxy on Windows for testing).

## What it is

A thin native shell that loads your Mellow server as a web app:

1. First launch shows a **Connect screen** (styled like Mellow's login): enter the
   server address (`192.168.1.5`, `192.168.1.5:6767`, or a full URL — default port 6767),
   or pick one discovered automatically on your LAN.
2. The window navigates in place to the server's normal **login page** — the web app
   itself is untouched and same-origin (chat, voice, uploads, WebSocket all as in a browser).
3. The server address is remembered (`%APPDATA%\dev.mellow.client\settings.json`, up to 5,
   most recent first). If the saved server is offline, the Connect screen comes back.

## Desktop extras

- **Tray**: closing the window hides it to the tray (left-click toggles, menu: Open / Switch server / Quit); tray tooltip shows unread count
- **Native notifications** for DMs/mentions (respects the app's DND + notification settings); only while the desktop client is running
- **Single instance**: launching again focuses the existing window
- Self-signed server certs are accepted by the embedded WebView2 (LAN trust model)

## Develop

```bash
npm install
npm run dev      # tauri dev (first run compiles Rust — slow once)
```

## Build the installer

```bash
npm run build    # emits src-tauri/target/release/bundle/nsis/Mellow_x64-setup.exe
```

Requires: Rust (rustc/cargo), WebView2 runtime (preinstalled on Win 10/11), VS Build Tools.

## Installing prebuilt installers (from GitHub Releases)

All installers are **unsigned** (no paid Apple/Windows dev certs). First launch needs
one manual bypass per OS:

- **Windows** — run `Mellow_x64-setup.exe` normally (may show SmartScreen → *More info* → *Run anyway*).
- **macOS** — download `Mellow_*_universal.dmg`, open it, drag Mellow to Applications, then
  **right-click → Open** once (Gatekeeper). If still blocked: `xattr -cr /Applications/Mellow.app`.
- **Linux** — `sudo dpkg -i Mellow_*_amd64.deb` (then `sudo apt -f install` if deps are pulled),
  or `chmod +x Mellow_*.AppImage && ./Mellow_*.AppImage`. AppImage tray icon needs the
  GNOME *AppIndicator* extension; the app still runs windowed without it.

Browser users keep using the `https://` server URL directly — nothing here is required for that.

## Android Client

Mellow runs natively on Android via Tauri v2 Mobile with full feature parity:
- **LAN Chat & Voice Calls**: Full WebRTC voice calls and camera support over LAN.
- **Localhost Reverse Proxy**: Transparently tunnels the self-signed HTTPS server through `http://127.0.0.1:<port>` over loopback, ensuring Chromium/Android System WebView treats the origin as a secure context.
- **Keep-Alive Foreground Service (FGS)**: Runs a background keep-alive service (`MellowKeepAliveService`, type `remoteMessaging`) holding a persistent notification ("Mellow — connected") and a partial wake lock while connected. This prevents Android 12+ from freezing the process or suspending WebSocket networking when the screen is locked or idle in your pocket.
- **Zero Server Changes / No Cloud Dependency**: Operates entirely offline on LAN with zero reliance on Google Play Services or Firebase Cloud Messaging (FCM).
- **Navigation & Mobile UX**: System Back button minimizes the app without closing the background service. An injected "Change server" button cleanly disconnects, terminates the service, and returns to the saved server launcher.

### Toolchain Matrix

To build the Android client on Windows/Linux/macOS, install:
- **Java**: JDK 21 (Temurin / OpenJDK 21) with `JAVA_HOME` configured
- **Android SDK**: Platforms 34–36, `build-tools 35.0.0` or `36.0.0`
- **Android NDK**: NDK r27 (`27.0.12077973`) with `NDK_HOME` configured
- **Tauri Tooling**: `@tauri-apps/cli` 2.11.4 / Tauri 2.11.5 / wry 0.55.1
- **Rust Android Targets**:
  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
  ```

### Build & Signing Commands

#### 1. Setup Signing
Generate a self-signed release keystore (one-time):
```bash
keytool -genkeypair -keystore src-tauri/gen/android/mellow-release.keystore -alias mellow -keyalg RSA -keysize 2048 -validity 10000
```
Create `src-tauri/gen/android/keystore.properties` (ignored by git, see `keystore.properties.example`):
```properties
storeFile=mellow-release.keystore
storePassword=your_store_password
keyAlias=mellow
keyPassword=your_key_password
```

#### 2. Development & Emulators
Run on a connected device or Android Studio emulator (e.g. API 34+ x86_64):
```bash
npx tauri android dev
```

#### 3. Build Signed Release APKs
```bash
# Build universal release APK (all ABIs)
npx tauri android build --apk

# Output APK location:
# Mellow-universal-release.apk (copied automatically to repo root)
# Also in: src-tauri/gen/android/app/build/outputs/apk/universal/release/Mellow-universal-release.apk
```

### Sideloading & Installation Notes

1. **Install via ADB**:
   ```bash
   adb install -r Mellow-universal-release.apk
   ```
2. **Install via Device Storage (Files app)**:
   - Transfer `Mellow-universal-release.apk` to your phone (via USB, NAS, or local share).
   - Open your file manager, tap the APK, and allow "Install unknown apps" when prompted.
3. **OEM Battery Optimization**:
   - On first connect, Mellow will request exemption from battery optimizations. Tap **Allow** so Android doesn't throttle background LAN connectivity.
   - For aggressive OEM task killers (Xiaomi MIUI/HyperOS, Samsung OneUI, OnePlus/Oppo ColorOS), ensure Mellow is set to "No restrictions" or "Unrestricted" under app battery settings.
4. **WebView Compatibility**:
   - WebRTC voice calls require Chromium / Android System WebView ≥ 89 (which supports loopback secure contexts). Any device with Google Play auto-updates enabled satisfies this.



## Repo map

- `launcher/` — the connect screen (plain HTML/CSS/JS)
- `src-tauri/` — Rust shell: commands (`connect`, `scan_lan`, `desktop_notify`, …), tray, NSIS bundling
- `SPIKE.md` — verification checklist / results for webview behaviors

Note: the dev/release binary is `mellow-client.exe`; the installed app is **Mellow.exe**
(start menu / desktop, per `productName`). Uninstalling removes only app files — chat data
lives on the server, and `%APPDATA%\dev.mellow.client` (saved servers) is left in place.
