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
# or with hot-reload:
dx serve --package omnibus
```

Opens at `http://127.0.0.1:3000`. Override with env vars:

| Variable | Default |
|---|---|
| `PORT` | `3000` |
| `DATABASE_URL` | `sqlite://omnibus.db?mode=rwc` |

## Running the mobile app

### iOS Simulator

Requires macOS with Xcode and at least one iOS Simulator installed.

```bash
dx serve --platform ios --package omnibus-mobile
```

The app connects to `http://127.0.0.1:3000` by default — start the server first.

### Android Emulator

Requires the Android SDK and NDK. If you have Android Studio, install the NDK via **Tools → SDK Manager → SDK Tools → NDK (Side by side)**. The dev shell auto-detects `ANDROID_NDK_HOME` on entry.

```bash
dx serve --platform android --package omnibus-mobile
```

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
