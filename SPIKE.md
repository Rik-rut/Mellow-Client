# Spike — WebView2 behavior checklist

Programmatic results are filled in as builds pass. Items marked [CLICK] need you in the app.

## Verified automatically (build/compile stage)
- [x] Rust toolchain: rustc 1.98.1, VS 2022, WebView2 runtime present
- [x] Server reachable over self-signed HTTPS (HTTP 200 on https://localhost:6767)
- [x] `cargo check` clean, zero warnings
- [x] NSIS installer builds: `src-tauri/target/release/bundle/nsis/Mellow_0.1.0_x64-setup.exe` (2.3 MB)
- [x] App binary launches and stays alive (window title "Mellow", no startup panic; tray, settings and single-instance init all succeeded)
- [x] Native icons regenerated from the REAL `mellow.svg` artwork (assets/mellow.svg → 1024px PNG via headless Chrome → `tauri icon`) — exe, title bar, taskbar, tray, Start menu, installer
- [x] "change server" rail button now injected via native `initialization_script` + `mellow-desktop://` navigation intercept — independent of remote-origin IPC (custom commands are ACL-blocked on remote pages in Tauri v2.11; verified in source)
- [ ] LAN discovery requires the server repo changes + a **server restart** to go live (the currently-running server predates them)

## [CLICK] — run `npm run dev` (or install the exe), then:
1. [ ] Connect screen: type `192.168.x.x:6767` → lands on Mellow login (no cert error)
2. [ ] Login works; stays logged in after closing the app (X = tray) and reopening
3. [ ] Voice join: mic permission prompt appears and audio works
4. [ ] Screen share button visible → share picker opens → remote sees frames
       (if broken on WebView2: hide feature on desktop and note here — decision point)
5. [ ] Upload an image in chat — shows inline
6. [ ] DM received while window hidden → Windows notification + tray tooltip count
7. [ ] DND status suppresses notifications
8. [ ] Second launch focuses existing window (single instance)
9. [ ] Tray "Switch server" returns to Connect screen; saved server offline at boot
       → Connect screen with warning banner
10. [ ] After 24h / server restart: reconnect works, token still valid

## Notes
- `--ignore-certificate-errors` is passed to WebView2 (whole-environment flag): the
  client accepts any TLS cert. Acceptable for trusted LAN use; a local-CA flow is a
  planned follow-up (Phase 3 cert work in the server repo).
- Remote-origin IPC (window.__TAURI__ on the server page) enables native notify +
  badge sync. If it does not activate, notifications silently fall back to in-app
  sounds only — verify via checklist item 6.
