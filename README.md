# Mellow Desktop Client

Tauri v2 desktop client for self-hosted [Mellow](../local-chat) LAN chat servers.
Windows-first (WebView2); the same core is designed to extend to macOS/Linux later.

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

## Repo map

- `launcher/` — the connect screen (plain HTML/CSS/JS)
- `src-tauri/` — Rust shell: commands (`connect`, `scan_lan`, `desktop_notify`, …), tray, NSIS bundling
- `SPIKE.md` — verification checklist / results for webview behaviors

Note: the dev/release binary is `mellow-client.exe`; the installed app is **Mellow.exe**
(start menu / desktop, per `productName`). Uninstalling removes only app files — chat data
lives on the server, and `%APPDATA%\dev.mellow.client` (saved servers) is left in place.
