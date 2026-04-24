# CLAUDE.md

Guidance for Claude Code when working in this repo. This file is an index — detailed rules and recipes live in [.claude/](.claude/).

Omnibus is a self-hosted ebook/audiobook library (see [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md)). The current counter app is a placeholder.

## Rules

Numbered rules in [.claude/rules/](.claude/rules/), applied in order. Follow them mechanically.

- [01-dev-environment.md](.claude/rules/01-dev-environment.md) — always work inside `nix develop`; env vars the shellHook sets.
- [02-error-handling.md](.claude/rules/02-error-handling.md) — `thiserror` for domain errors, `anyhow` for handlers.
- [03-unit-testing.md](.claude/rules/03-unit-testing.md) — inline `#[cfg(test)]`, `oneshot` for handlers, coverage expectations.
- [04-playwright.md](.claude/rules/04-playwright.md) — full E2E conventions (selectors, fixtures, `expectMutation`, error paths).
- [98-keep-skills-fresh.md](.claude/rules/98-keep-skills-fresh.md) — update skills when the code they reference changes.
- [99-end-of-session.md](.claude/rules/99-end-of-session.md) — end-of-session checklist (docs sync, fmt/clippy, coverage, line-count cap).

**Line-count cap:** every file in `CLAUDE.md` / `.claude/` stays under ~200 lines. Split by topic when it grows past that — enforced by rule 99.

## Skills

Auto-discoverable skills in [.claude/skills/](.claude/skills/) — Claude Code loads each `SKILL.md` automatically, and each is invokable via `/<name>` (e.g. `/jj-basics`).

- [add-backend-route](.claude/skills/add-backend-route/SKILL.md) — adding an Axum page or API endpoint end-to-end.
- [add-playwright-flow](.claude/skills/add-playwright-flow/SKILL.md) — adding a new E2E spec.
- [jj-basics](.claude/skills/jj-basics/SKILL.md) — fetch / new / describe / bookmark / push.
- [jj-workspaces](.claude/skills/jj-workspaces/SKILL.md) — parallel agent work on one repo.
- [jj-advanced](.claude/skills/jj-advanced/SKILL.md) — squash / rebase / abandon / undo / op log / conflicts.

## Architecture

Cargo workspace with five crates:

- **`shared/`** (`omnibus-shared`) — serde types shared across every target (`Settings`, `ValueResponse`, `LibraryContents`, `LibrarySection`). No Dioxus / axum / sqlx deps.
- **`db/`** (`omnibus-db`) — server-side data layer: SQL migrations, SQLite pool init, the normalized query layer, and the indexing pipeline (scanner → ebook metadata extraction → atomic per-library upsert). Consumed by both `server/` (REST handlers) and `frontend/` (server-function bodies). Holds all sqlx / tokio / epub / anyhow dependencies on the server side.
- **`frontend/`** (`omnibus-frontend`) — Dioxus UI + server-function wire layer (`rpc.rs`). Feature-gated:
  - `web` — WASM client build (used by `server/` when `dx serve --platform web` builds it for WASM).
  - `mobile` — Native Dioxus build; uses `reqwest` against `/api/*` REST routes.
  - `server` — SSR/native build; pulls in `omnibus-db` and compiles server-function bodies. Name is hardcoded by the dioxus_fullstack_macro — can't be renamed.
- **`server/`** (`omnibus`) — **unified Dioxus fullstack binary**. Built twice by `dx serve`: once native (feature `server`) for the axum backend + SSR, once WASM (feature `web`) for the hydrated client. Hosts the hand-written `/api/*` REST router for mobile. Depends directly on `omnibus-db`.
- **`mobile/`** (`omnibus-mobile`) — thin Dioxus Native shell (~16 lines) that injects `ServerUrl` context and launches `omnibus_frontend::App`.

Default `cargo build` / `clippy` covers `server`, `shared`, `frontend` only. Mobile is excluded via workspace `default-members` because its `mobile` feature is mutually exclusive with `web`; build it explicitly: `cargo build -p omnibus-mobile`.

**Web request flow (fullstack):** browser → axum serves SSR'd HTML + WASM bundle → hydration → signal effects call Dioxus server functions (`#[get]`/`#[post]` in `frontend/src/rpc.rs`) at `/api/rpc/*` → same handlers execute server-side against the SQLite pool via an `axum::Extension<SqlitePool>` layer.

**Mobile data flow:** Dioxus signal/effect → `reqwest` call to `/api/*` (hand-written handlers in `server/src/backend.rs`) → signal update → re-render. Mobile deliberately does **not** use the `/api/rpc/*` server functions.

**Database:** schema ships as numbered SQL migrations under [db/migrations/](db/migrations/), embedded via `sqlx::migrate!` and run on pool init in `omnibus_db::init_db`. Applied versions are recorded in the `_sqlx_migrations` table. Add new migrations as `NNNN_description.sql` — never edit an applied file. All tests use `sqlite::memory:` for isolation; the migrator runs against them the same as production.

**Server URL (mobile):** hardcoded to `http://127.0.0.1:3000` in `mobile/src/main.rs` via `use_context_provider`. Will become a first-launch setup screen.

### shared/src/

```
lib.rs              — Settings, ValueResponse, LibraryContents, LibrarySection, EbookMetadata, EbookLibrary
```

### db/src/

```
lib.rs              — re-exports queries::*; pub mod ebook/indexer/queries/scanner
queries.rs          — pool init, schema, query layer (list_books, settings, covers, taxonomy…)
scanner.rs          — library directory scanning
ebook.rs            — EPUB OPF metadata + cover extraction
indexer.rs          — scan → DB indexing, staleness checks
migrations/         — numbered SQL migrations embedded via sqlx::migrate!
```

### frontend/src/

```
lib.rs              — Route, App, styles, ScreenLayout (feature-gated)
data.rs             — Feature-gated data transport (mobile=reqwest, web/server=rpc)
rpc.rs              — #[get]/#[post] server functions (mounted by dioxus::server::router); server bodies call into omnibus_db
pages/{landing,settings,book_detail,auth}.rs  — auth.rs hosts LoginPage + RegisterPage
components/{top_nav,bottom_nav}.rs  — feature = web / mobile respectively
```

### server/src/

```
main.rs             — dioxus::launch (WASM) / dioxus::serve (native); mounts auth_router + rate-limit + origin-check
lib.rs              — re-exports backend + auth under `server` feature for tests
backend.rs          — /api/* REST router (mobile-facing) + integration tests
auth/mod.rs         — /api/auth/{register,login,logout,me} + AuthUser/AdminUser extractors + CSRF origin-check
auth/gate.rs        — top-level middleware gating /api/* (pass-through for /api/auth/*)
auth/rate_limit.rs  — in-memory per-IP fixed-window counter for login/register
auth/strategy.rs    — AuthStrategy trait + PasswordStrategy (OIDC/WebAuthn fit the same shape)
auth/boot.rs        — OMNIBUS_INITIAL_ADMIN recovery hook (promotes named user to admin)
```

### ui_tests/playwright/

```
tests/
  flows/            — one *.spec.ts per user flow
  utils/            — cross-flow helpers (nav, api mutation assertions)
  fixtures/         — extended `test` / `expect` exports
```

### mobile/src/

```
main.rs             — dioxus::launch, ServerUrl context, wraps omnibus_frontend::App
```

## Quick commands

```bash
# Multiplexed dev stack (server + ios + android + playwright panes)
just serve                                                  # Zellij
just serve-pc                                               # process-compose

# Fullstack dev (serves SSR + WASM hydration at http://localhost:8080 by default)
dx serve --platform web -p omnibus

# Server only (native backend, no WASM bundle)
cargo run -p omnibus                                        # start at http://0.0.0.0:3000
cargo test -p omnibus                                       # /api/* REST integration tests
cargo test -p omnibus-db                                    # db + ebook + scanner tests
cargo clippy                                                # lint default-members crates
cargo fmt                                                   # format all crates

# Playwright E2E (server must be running)
cd ui_tests/playwright && npm install                       # first time
cd ui_tests/playwright && npx playwright test               # run all

# Mobile
cargo build -p omnibus-mobile
xcrun simctl boot "iPhone 17" 2>/dev/null; dx serve --platform ios --package omnibus-mobile
dx serve --platform android --package omnibus-mobile
adb reverse tcp:3000 tcp:3000                               # after Android emulator boots
```

## Project direction

See [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md) for the phased roadmap (foundations, browse/discovery, reading/listening, personalization, device sync, admin, mobile).
