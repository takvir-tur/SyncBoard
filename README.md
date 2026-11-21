# ClipSync (SyncBoard)

Ultra-lightweight, zero-config, end-to-end encrypted shared clipboard across Windows, macOS, and Android backed by a \$0 Cloudflare Workers + Durable Objects relay.

## Architecture

- **`relay/`** — Cloudflare Worker + Durable Object (`SyncRoom`) using the **WebSocket Hibernation API** (`0` idle CPU) and Durable Object SQLite (`pins` table capped at 25 encrypted blobs per room). Never sees plaintext.
- **`core/` (`clipsync_core`)** — Rust shared engine (`cdylib`, `staticlib`, `rlib`) compiled with `opt-level = "z"`, `lto = true`, `strip = true`, `panic = "abort"`.
  - Key derivation: `RoomID = BLAKE3("room", ClusterKey)` and `EncryptionKey = BLAKE3("e2ee", ClusterKey)`.
  - Frame encryption: `ChaCha20-Poly1305` with authenticated 26-byte header (`version | tag | item_id | timestamp_ms`).
  - Echo suppression: `BLAKE3` rolling hash ring buffer.
  - Media compression: lossless PNG for small screenshots, WebP/JPEG fallback capped at `8 MiB`.
  - Persistence: encrypted `pins.bin` + `config.bin`, C-ABI (`core/include/clipsync.h`), and Android JNI (`core/src/jni_bridge.rs`).
- **`desktop/windows/`** — Pure Rust Win32 tray app using `AddClipboardFormatListener` (`WM_CLIPBOARDUPDATE`) on a hidden `HWND_MESSAGE` window (`0%` idle CPU), `tray-icon` with a "Start at Login" (`HKCU\...\Run`) toggle enabled on first launch, and a GDI QR pairing window.
- **`desktop/macos/`** — Native Swift menu-bar agent (`LSUIElement = true`) bridging to `clipsync_core`, using a coalesced `DispatchSource` timer (500 ms interval, 200 ms App Nap leeway), `SMAppService.mainApp` login auto-start toggle, and `NSPopover`.
- **`android/`** — Native Kotlin companion app with JNI bridge, `ForegroundService` (`dataSync`), `BOOT_COMPLETED` auto-start receiver, optional zero-tap Shizuku `IClipboard` background auto-sync, Quick Settings `TileService`, persistent notification **Sync** / **Pin** actions, Share Sheet (`ACTION_SEND`) receiver, translucent 30 ms `FocusCaptureActivity`, QR generation, and CameraX + MLKit QR scanner.

## Quickstart

### 1. Deploy the \$0 Cloudflare Relay (Optional — Pre-Configured)

All prebuilt binaries default to `wss://clipsync-relay.takvir-tur.workers.dev` and work out of the box. To host your own relay:

```bash
cd relay
npm install
npx wrangler deploy
```

Set `CLIPSYNC_RELAY_URL=wss://<your-worker>.workers.dev` to override the baked-in relay URL on desktop.

### 2. Test & Build the Rust Core

```bash
cargo test -p clipsync_core
cargo build --release -p clipsync_core
```

### 3. Build Desktop & Android Clients

- **Windows:**
  ```bash
  cargo build --release -p clipsync_windows --bin clipsync
  ```
- **macOS (Universal `.dmg`):**
  ```bash
  ./desktop/macos/build_dmg.sh
  ```
- **Android (`.apk`):**
  ```bash
  cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o android/app/src/main/jniLibs build --release --manifest-path core/Cargo.toml
  cd android && gradle assembleRelease
  ```
