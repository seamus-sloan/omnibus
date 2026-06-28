# omnibus

Self-hosted ebook and audiobook library — the Plex/Jellyfin for your book collection. Built with Rust (Axum + Dioxus), SQLite, and a native iOS/Android app.

> **Status:** early development. Foundations and browse/discovery have shipped and reading/listening is in progress — landing grid/table, EPUB reader, audiobook player, command-palette search, and auth are live. See [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md) for the full roadmap.

## Running

> **Just want to deploy it?** See [docs/docker.md](docs/docker.md) for a
> Jellyfin-style `docker compose` setup (bind-mount your library, durable
> state in `/config`, cache in `/cache`). The Nix flow below is for development.

This project utilizes [Nix](https://wiki.nixos.org/wiki/NixOS_Wiki) to save all dependencies.
```bash
# Launch the nix shell (slim default — daily cargo/clippy/test)
nix develop

# Launch the nix shell manually (and keep your shell)
nix develop --command zsh

# Auto-load the nix shell with direnv
direnv allow # Only necessary once
```

The flake exposes purpose-built shells so daily work doesn't pull Playwright + mobile + audit tooling. The default direnv-loaded shell is `default` (slim); opt in to a heavier shell only when you need it:

| Shell | When to use |
|---|---|
| `default` | Daily `cargo` / editor — what direnv auto-loads |
| `.#web` | `dx serve --platform web`, `just dev-up`, anything that bundles WASM |
| `.#e2e` | `npx playwright test` (Chromium bundle is here) |
| `.#mobile` | Android / iOS builds (rust cross-targets + JDK + Android NDK detect) |
| `.#audit` | `cargo audit` / `cargo deny` |

`just serve`, `just serve-pc`, `just dev-up`, and `just dev-bounce` self-wrap in the right shell, so they work from `default`.

Multiplexers with ZelliJ and Process-Compose both leverage the same `.justfile` to launch all of the different platforms here (frontend, mobile, server, playwright).
```bash
# Launch with ZelliJ
just serve

# Launch with Process-Compose
just serve-pc
```

| Variable | Default |
|---|---|
| `PORT` | `3000` |
| `DATABASE_URL` | `sqlite://omnibus.db?mode=rwc` |

## Running the mobile app

The mobile app connects to `http://127.0.0.1:3000` — start the server first (`just serve` handles this). The iOS and Android panes inside `just serve` run the commands below for you; the one-time setup steps still apply.

### iOS Simulator

Requires macOS with Xcode and at least one iOS Simulator installed (add one via **Xcode → Window → Devices and Simulators**). 

The iOS pane in the multiplexer will be able to launch the simulator and install the app without any other additional commands.

### Android Emulator
> To launch the android emulator & app, use the pane/tab inside of the `just serve` commands. You may need to set the environment variables below.

1. **Install Android Studio** — [developer.android.com/studio](https://developer.android.com/studio)

2. **Install the NDK** — in Android Studio: **Tools → SDK Manager → SDK Tools tab → check "NDK (Side by side)" → Apply**

3. **Create an emulator** — in Android Studio: **Tools → Device Manager → Create Virtual Device**, pick a device with a recent API level (API 33+ recommended), then start it.

4. **Enter the Nix mobile dev shell** — the `mobile` shellHook auto-detects `ANDROID_HOME` and `ANDROID_NDK_HOME` from the standard Android Studio install paths:

   ```bash
   nix develop .#mobile --command zsh
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

5. After the app is running, any issues where the app is unable to communicate with the server can be resolved by running `adb reverse tcp:3000 tcp:3000` to properly set the localhost.

## Tests

```bash
# [UNIT TESTS] Running tests
cargo test -p omnibus

# [WEB UI TESTS] Running tests (Requires server & frontend to be running)
cd ui_tests/playwright
npm install          # First time only. Chromium is provided through Nix.
npx playwright test
```

*Mobile UI tests will be done later and will be using [Maestro](https://github.com/mobile-dev-inc/maestro)*