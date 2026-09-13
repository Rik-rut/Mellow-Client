# Mellow Client

Mellow Client is an app for your computer and phone that connects to your private [Mellow Server](https://github.com/Rik-rut/Mellow-Server). It lets you chat and make voice calls with other people on your local home or office network without relying on cloud services.

## What is Mellow?

Mellow is a private chat system designed to run on your own local network (LAN). Instead of sending your messages and calls through third-party servers on the internet, Mellow keeps everything inside your private network.

To use Mellow, you need two parts:
1. A running [Mellow Server](https://github.com/Rik-rut/Mellow-Server) hosted on a computer in your network.
2. The Mellow Client app (this application) installed on your Windows, Mac, Linux, or Android device.

## Key Features

- Private Chat: Send messages and files directly through your local network.
- Voice and Video Calls: Talk with other members with working microphone and camera support.
- Automatic Discovery: Automatically finds Mellow servers running on your local network.
- Remembers Your Servers: Saves your server address so you do not have to type it every time.
- Desktop Notifications: Receive message alerts while the app is open or running in the system tray.
- Stays Connected on Android: Keeps you connected in the background so you do not miss messages when your phone is locked.

## How to Install

Download the installer for your system from the Releases section of this repository.

Because Mellow Client is self-published and does not use expensive commercial developer certificates, your operating system may show a standard safety prompt during the first install. Follow the simple steps below for your device.

### Windows

1. Download the installer named `Mellow_x64-setup.exe`.
2. Double-click the file to begin installation.
3. If Windows SmartScreen appears saying "Windows protected your PC":
   - Click **More info**.
   - Click **Run anyway**.
4. Follow the setup wizard to finish installing. You can launch Mellow from your Start Menu or Desktop shortcut.

### Mac (macOS)

1. Download `Mellow_*_universal.dmg`.
2. Double-click the `.dmg` file to open it, then drag the Mellow icon into your **Applications** folder.
3. Open your Applications folder.
4. Right-click (or Control-click) the Mellow icon and select **Open**.
5. When prompted by macOS, click **Open** to confirm. (You only need to do this the very first time you open the app).

### Linux

You can install Mellow using either a package or an AppImage:

- **Debian / Ubuntu (.deb)**:
  Download `Mellow_*_amd64.deb` and install it through your package manager, or run:
  ```bash
  sudo dpkg -i Mellow_*_amd64.deb
  ```
- **AppImage**:
  Download `Mellow_*.AppImage`, make it executable, and run it:
  ```bash
  chmod +x Mellow_*.AppImage
  ./Mellow_*.AppImage
  ```

### Android

1. Download `Mellow-universal-release.apk` directly to your phone (or transfer it from your computer).
2. Open your phone's file manager or downloads list and tap the `.apk` file.
3. If your phone asks for permission to install apps from this source, tap **Settings** and turn on **Allow from this source**, then continue the installation.
4. **Important battery setting**: When you first connect, Mellow will ask for permission to ignore battery optimizations. Choose **Allow**. This ensures Android does not turn off the app when your screen is locked, allowing you to receive notifications reliably. On phones with strict battery savers (such as Xiaomi, Samsung, or OnePlus), check your phone's Settings and set Mellow's battery usage to "Unrestricted".

## Getting Started

1. Open the Mellow app.
2. On the **Connect** screen:
   - If your server is discovered automatically on your network, click its address in the list.
   - Otherwise, enter the server address manually (for example, `192.168.1.5` or `192.168.1.5:6767`).
   - For Cloudflare Tunnels or custom domains, always add `:443` at the end (for example, `chat.yourdomain.com:443`). Without `:443`, the client automatically defaults to local port 6767.
3. Click **Connect**.
4. Log in with your Mellow username and password.

The app will remember your server for future launches. If you ever need to change servers, you can use the tray menu on desktop or the "Change server" button on mobile.

---

## For Developers and Advanced Users

This section contains technical information for building and modifying Mellow Client from source code.

### Desktop Development Setup

Mellow Client is built with Tauri v2 (Rust backend with a web frontend).

Prerequisites:
- Node.js and npm
- Rust (`rustc` and `cargo`)
- WebView2 runtime (pre-installed on Windows 10 and 11)
- C++ build tools (Visual Studio Build Tools on Windows, Xcode tools on macOS, or build-essential on Linux)

Commands:

```bash
# Install dependencies
npm install

# Run in development mode
npm run dev

# Build the desktop installer
npm run build
```

The compiled installer will be located in:
`src-tauri/target/release/bundle/`

### Android Development Setup

Prerequisites:
- JDK 21 (Temurin or OpenJDK 21) with `JAVA_HOME` set
- Android SDK (Platforms 34 to 36, build-tools 35.0.0 or 36.0.0)
- Android NDK (NDK r27 / `27.0.12077973`) with `NDK_HOME` set
- Rust Android targets:
  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
  ```

#### Keystore Setup

Generate a release keystore (one-time setup):
```bash
keytool -genkeypair -keystore src-tauri/gen/android/mellow-release.keystore -alias mellow -keyalg RSA -keysize 2048 -validity 10000
```

Create `src-tauri/gen/android/keystore.properties` (based on `keystore.properties.example`):
```properties
storeFile=mellow-release.keystore
storePassword=your_store_password
keyAlias=mellow
keyPassword=your_key_password
```

#### Android Commands

```bash
# Run on an emulator or connected device
npx tauri android dev

# Build the release APK
npx tauri android build --apk
```

The APK file will be saved at the repository root as `Mellow-universal-release.apk`.

### Architecture Notes

- **Localhost Reverse Proxy**: On macOS, Linux, and Android, the client proxies self-signed server HTTPS traffic through `http://127.0.0.1:<port>`. This allows WebRTC voice calls and camera access to work in WebKit and Android WebView without requiring a custom certificate authority.
- **Android Background Service**: A foreground service (`MellowKeepAliveService`) runs on Android with a persistent notification and wake lock while connected, preventing the operating system from suspending WebSocket connections when the device is idle.
- **Zero Cloud Dependency**: Operates entirely over local network connections without Firebase Cloud Messaging or third-party push notification services.

### Project Layout

- `launcher/`: Connect screen frontend (HTML, CSS, JavaScript).
- `src-tauri/`: Native application shell written in Rust, handling window management, system tray, local network scanning, and notifications.
- `src-tauri/gen/android/`: Android project files and background service implementation.
