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

## Running the full dev stack

Inside the Nix dev shell, one command brings up server + iOS + Android + Playwright panes:

```bash
just serve          # Zellij — 4 tabs, server auto-runs; press Enter in other panes to start them
just serve-pc       # process-compose — TUI with logs; F7 to start a disabled process, F10 to quit
```

Prefer `just serve` if you like tab-based navigation (`Alt+<N>` to jump, `Ctrl-q` to quit). Prefer `just serve-pc` if you want a single scrollable log view.

If you only need the server (no mobile or E2E):

```bash
cargo run -p omnibus
```

## Running the mobile app

The mobile app connects to `http://127.0.0.1:3000` — start the server first (`just serve` handles this). The iOS and Android panes inside `just serve` run the commands below for you; the one-time setup steps still apply.

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

With the emulator booted, launch `just serve` (or `just serve-pc`) and use the Android pane. Then, from any shell inside the dev shell:

```bash
adb reverse tcp:3000 tcp:3000
```

The `adb reverse` step is required because `127.0.0.1` inside the Android emulator refers to the emulator itself, not your Mac. Re-run it any time you restart the emulator.

## Tests

```bash
cargo test -p omnibus          # all server unit + integration tests
```

### E2E tests (Playwright)

1. Start the server: `cargo run -p omnibus` (or use the `playwright` pane inside `just serve`).
2. Run:

```bash
cd ui_tests/playwright
npm install          # first time only
npx playwright test
```

Chromium is provided by Nix via `PLAYWRIGHT_BROWSERS_PATH` — do **not** run `npx playwright install`.
