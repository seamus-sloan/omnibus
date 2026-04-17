# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.
As files change, this document should be kept up-to-date.
As preferences change, this document should be kept up-to-date.

## Development environment

All system dependencies (Rust toolchain, SQLite, pkg-config, OpenSSL, Node.js, Android SDK/NDK, JDK) are provided by Nix. Always work inside the dev shell:

```bash
nix develop --command zsh   # preferred — keeps your shell prompt intact
nix develop                  # also works; spawns a bash subshell
```

`DATABASE_URL` is preset by the shell hook to `sqlite://omnibus.db?mode=rwc`. Override `PORT` (default `3000`) if needed.

## Common commands

```bash
# Server
cargo run -p omnibus                                        # start the server at http://0.0.0.0:3000
cargo test -p omnibus                                       # run all server tests
cargo test -p omnibus <test_name>                           # run a single test by name
cargo test -p omnibus --features e2e -- --ignored           # run Playwright E2E tests (requires running server)
dx serve --port 3000 --package omnibus                      # run server with hot-reload, devserver fixed at port 3000
cargo clippy                                                # lint all crates
cargo fmt                                                   # format all crates

# Mobile
cargo build -p omnibus-mobile                               # build mobile app
dx serve --platform ios --package omnibus-mobile            # run in iOS Simulator (requires Xcode)
dx serve --platform android --package omnibus-mobile        # run in Android Emulator (requires Android SDK)
adb reverse tcp:3000 tcp:3000                               # forward emulator port 3000 → host port 3000 (run after emulator boots)
```

## Running server + mobile simultaneously (with hot-reload)

The mobile app is hardcoded to `http://127.0.0.1:3000`. To develop both at once:

```bash
# Terminal 1 — server with hot-reload, devserver proxy fixed at port 3000
dx serve --port 3000 --package omnibus

# Terminal 2 — mobile (Android example)
dx serve --platform android --package omnibus-mobile

# Terminal 3 — once emulator is running
adb reverse tcp:3000 tcp:3000
```

`dx serve` picks a random port by default — `--port 3000` pins the devserver proxy, which forwards API requests to the actual server binary. Never use `dx serve` without `--port` when the mobile app needs to connect.

## Architecture

This is a Cargo workspace with two crates:

- **`server/`** (`omnibus`) — Axum SSR server. Dioxus is used only as a templating engine; all interactivity is plain JavaScript.
- **`mobile/`** (`omnibus-mobile`) — Dioxus Native mobile app. Communicates with the server via its JSON API.

**Server request flow:** Axum handler → `db/` query → Dioxus SSR component renders HTML string → `Html(...)` response. JSON API routes skip SSR and return `Json(...)` directly.

**Mobile data flow:** Dioxus signal/effect → `reqwest` call to `/api/*` → signal update → re-render.

**Database:** Schema is created inline at startup in `db::initialize_schema`. There is no migrations framework yet. All tests use `sqlite::memory:` for isolation.

**Server URL (mobile):** Hardcoded to `http://127.0.0.1:3000` in `mobile/src/main.rs` via `use_context_provider`. Will become a user-configurable first-launch setup screen.

## Module structure

### server/src/

```
main.rs
lib.rs
backend.rs          — Axum router + AppState + handlers
db.rs               — pool init, schema, queries
frontend/
  mod.rs            — Route enum, App component, render_document, styles, SSR tests
  pages/
    mod.rs
    landing.rs      — LandingPage component
    settings.rs     — SettingsPage component
  components/
    mod.rs
    nav.rs          — TopNav component
tests/
  e2e_playwright.rs — browser tests, feature-gated behind `--features e2e`
```

### mobile/src/

```
main.rs           — dioxus::launch, Route enum, App + screen components, ServerUrl context, CSS styles
pages/
  mod.rs
  landing.rs      — LandingPage with live API calls via reqwest
  settings.rs     — SettingsPage
components/
  mod.rs
  nav.rs          — BottomNav with dioxus-router Links
```

## Error handling

- **Domain/library errors:** use `thiserror` to define typed error enums per module (e.g. `DbError`, `ScanError`). Each variant should have a meaningful `#[error("...")]` message.
- **Application-level propagation:** use `anyhow` in handlers and top-level code where the specific error type doesn't need to be matched.
- `#[error(transparent)]` + `#[from]` for wrapping lower-level errors (sqlx, std::io, etc.).

```rust
// Domain error — in a library module
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("file not found: {path}")]
    NotFound { path: PathBuf },
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// Handler — anyhow for propagation, no need to enumerate variants
async fn trigger_scan(State(s): State<AppState>) -> Result<StatusCode, anyhow::Error> {
    scanner::scan(&s.pool).await?;
    Ok(StatusCode::OK)
}
```

## Testing

Testing is a first-class requirement. Every meaningful behavior should have a test at the **lowest applicable level**:

- **Unit tests:** inline `#[cfg(test)]` modules in the same file as the code under test. This is the default for all logic in `db/`, `scanner/`, and `frontend/`.
- **Integration tests:** inline `#[cfg(test)]` in `backend.rs`, using `tower::ServiceExt::oneshot` to test full request/response cycles against an in-memory DB.
- **E2E tests:** Playwright tests in `tests/e2e_playwright.rs`, feature-gated with `--features e2e`, for user-facing flows.

Coverage expectations:
- All `db/` functions: happy path + not-found/missing + constraint violations
- All `backend/` handlers: 200 success, 4xx client errors, 5xx DB failure paths
- All `frontend/` components with logic: rendered output contains expected content
- New features must not ship without tests covering their acceptance criteria from `ROADMAP.md`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_book_returns_correct_record() { ... }

    #[tokio::test]
    async fn get_book_returns_not_found_for_missing_id() { ... }

    #[tokio::test]
    async fn get_book_returns_error_on_db_failure() { ... }
}
```

## CLAUDE.md updates

Update this file automatically at the end of any session where:
- A new module or subdirectory is introduced
- A new dependency is added to `Cargo.toml`
- A new environment variable or configuration key is used
- A convention is established or changed (error handling, test patterns, etc.)

## Project direction

This repo is being built into a self-hosted ebook/audiobook library (see `ROADMAP.md`). The current counter app is a placeholder. The planned stack additions (OPDS feed, epub/m4a scanning, Dioxus Native mobile app, etc.) are all documented in `ROADMAP.md` along with the full intended database schema.
