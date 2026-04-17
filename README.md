# omnibus

Self-hosted ebook and audiobook library — the Plex/Jellyfin for your book collection. Built with Rust (Axum + Dioxus), SQLite, and a native iOS/Android app.

> **Status:** early development. The current UI is a placeholder counter; see [ROADMAP.md](ROADMAP.md) for planned features.

## Prerequisites

All system dependencies are provided by Nix (Rust toolchain, SQLite, Node.js, `dx` CLI, iOS/Android cross-compilation targets):

```bash
nix develop          # enter the dev shell (bash)
nix develop --command zsh   # preferred — keeps your zsh prompt
```

Everything below assumes you're inside the dev shell.

## Running the server

```bash
cargo run -p omnibus
```

Opens at `http://0.0.0.0:3000`. Override with env vars:

| Variable | Default |
|---|---|
| `PORT` | `3000` |
| `DATABASE_URL` | `sqlite://omnibus.db?mode=rwc` |

## Running the server (with hot-reload)

When developing alongside the mobile app, pin the devserver port so the mobile app can always reach it:

```bash
dx serve --port 3000 --package omnibus
```

Without `--port 3000`, `dx serve` picks a random port each run and the mobile app won't connect.

## Running the mobile app

The app connects to `http://127.0.0.1:3000` — always start the server first.

### iOS Simulator

Requires macOS with Xcode and at least one iOS Simulator installed (add one via **Xcode → Window → Devices and Simulators**).

```bash
xcrun simctl boot "iPhone 17" 2>/dev/null
dx serve --platform ios --package omnibus-mobile
```

### Android Emulator

#### One-time setup

1. **Install Android Studio** — [developer.android.com/studio](https://developer.android.com/studio)

2. **Install the NDK** — in Android Studio: **Tools → SDK Manager → SDK Tools tab → check "NDK (Side by side)" → Apply**

3. **Create an emulator** — in Android Studio: **Tools → Device Manager → Create Virtual Device**, pick a device with a recent API level (API 33+ recommended), then start it.

4. **Enter the Nix dev shell** — the shellHook auto-detects `ANDROID_HOME` and `ANDROID_NDK_HOME` from the standard Android Studio install paths:

   ```bash
   nix develop --command zsh
   ```

   Verify the vars are set:

   ```bash
   echo $ANDROID_HOME       # e.g. /Users/<you>/Library/Android/sdk
   echo $ANDROID_NDK_HOME   # e.g. /Users/<you>/Library/Android/sdk/ndk/28.x.x
   ```

   If they're empty, set them manually (substituting your NDK version):

   ```bash
   export ANDROID_HOME=$HOME/Library/Android/sdk
   export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
   ```

#### Running

With the emulator booted, in separate terminals:

```bash
# Terminal 1 — server
dx serve --port 3000 --package omnibus

# Terminal 2 — Android app
dx serve --platform android --package omnibus-mobile

# Terminal 3 — forward emulator's localhost:3000 → host's localhost:3000
adb reverse tcp:3000 tcp:3000
```

The `adb reverse` step is required because `127.0.0.1` inside the Android emulator refers to the emulator itself, not your Mac. Re-run it any time you restart the emulator.

## Tests

```bash
cargo test -p omnibus          # all server unit + integration tests
```

### E2E tests (Playwright)

1. Start the server: `cargo run -p omnibus`
2. Run:

```bash
cargo test -p omnibus --features e2e -- --ignored
```
