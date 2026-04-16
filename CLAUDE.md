# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.
As files change, this document should be kept up-to-date.
As preferences change, this document should be kept up-to-date.

## Development environment

All system dependencies (Rust toolchain, SQLite, pkg-config, OpenSSL, Node.js) are provided by Nix. Always work inside the dev shell:

```bash
nix develop --command zsh   # preferred — keeps your shell prompt intact
nix develop                  # also works; spawns a bash subshell
```

`DATABASE_URL` is preset by the shell hook to `sqlite://omnibus.db?mode=rwc`. Override `PORT` (default `3000`) if needed.

## Common commands

```bash
cargo run                                        # start the server at http://127.0.0.1:3000
cargo test                                       # run all unit and integration tests
cargo test <test_name>                           # run a single test by name (substring match)
cargo test --features e2e -- --ignored           # run Playwright E2E tests (requires running server)
cargo clippy                                     # lint
cargo fmt                                        # format
```

## Architecture

This is a server-side-rendered full-stack Rust app. There is **no client-side Rust/Wasm** — Dioxus is used only as a templating engine on the server, and all interactivity is plain JavaScript.

**Request flow:** Axum handler → `db/` query → Dioxus SSR component renders HTML string → `Html(...)` response. JSON API routes skip SSR and return `Json(...)` directly.

**Database:** Schema is created inline at startup in `db::initialize_schema`. There is no migrations framework yet. All tests use `sqlite::memory:` for isolation.

**Frontend interactivity:** Embedded JS in `frontend/` handles DOM updates by calling the JSON API routes (`/api/*`) and patching specific element IDs.

## Module structure

Modules are organized as nested subdirectories, split by domain. When a file grows large, break it into a subdirectory with a `mod.rs` and focused child modules. The target structure as features are added:

```
src/
  main.rs
  lib.rs
  backend/
    mod.rs          — Axum router + AppState
    books.rs        — book route handlers
    libraries.rs    — library route handlers
    auth.rs         — login/register/logout handlers
    admin.rs        — admin-only route handlers
  db/
    mod.rs          — pool init, shared helpers
    books.rs        — book queries
    libraries.rs    — library queries
    users.rs        — user/session queries
  frontend/
    mod.rs          — render_document entry point
    pages/          — one file per page (landing, book detail, reader, etc.)
    components/     — shared UI components (nav, rating widget, etc.)
  scanner/
    mod.rs          — directory walker, orchestration
    epub.rs         — epub metadata + cover extraction
    audiobook.rs    — m4a metadata + chapter extraction
tests/
  e2e_playwright.rs — browser tests, feature-gated behind `--features e2e`
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
- **Integration tests:** inline `#[cfg(test)]` in `backend/` modules, using `tower::ServiceExt::oneshot` to test full request/response cycles against an in-memory DB.
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
