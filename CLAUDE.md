# CLAUDE.md

Guidance for Claude Code when working in this repo. This file is an index — detailed rules and recipes live in [.claude/](.claude/).

Omnibus is a self-hosted ebook/audiobook library (see [ROADMAP.md](ROADMAP.md)). The current counter app is a placeholder.

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

Cargo workspace with four crates:

- **`shared/`** (`omnibus-shared`) — serde types shared across every target (`Settings`, `ValueResponse`, `LibraryContents`, `LibrarySection`). No Dioxus / axum / sqlx deps.
- **`frontend/`** (`omnibus-frontend`) — Dioxus UI + the DB layer + server functions. Feature-gated:
  - `web` — WASM client build (used by `server/` when `dx serve --platform web` builds it for WASM).
  - `mobile` — Native Dioxus build; uses `reqwest` against `/api/*` REST routes.
  - `server` — SSR/native build; enables sqlx + tokio and compiles server-function bodies.
- **`server/`** (`omnibus`) — **unified Dioxus fullstack binary**. Built twice by `dx serve`: once native (feature `server`) for the axum backend + SSR, once WASM (feature `web`) for the hydrated client. Hosts the hand-written `/api/*` REST router for mobile.
- **`mobile/`** (`omnibus-mobile`) — thin Dioxus Native shell (~16 lines) that injects `ServerUrl` context and launches `omnibus_frontend::App`.

Default `cargo build` / `clippy` covers `server`, `shared`, `frontend` only. Mobile is excluded via workspace `default-members` because its `mobile` feature is mutually exclusive with `web`; build it explicitly: `cargo build -p omnibus-mobile`.

**Web request flow (fullstack):** browser → axum serves SSR'd HTML + WASM bundle → hydration → signal effects call Dioxus server functions (`#[get]`/`#[post]` in `frontend/src/rpc.rs`) at `/api/rpc/*` → same handlers execute server-side against the SQLite pool via an `axum::Extension<SqlitePool>` layer.

**Mobile data flow:** Dioxus signal/effect → `reqwest` call to `/api/*` (hand-written handlers in `server/src/backend.rs`) → signal update → re-render. Mobile deliberately does **not** use the `/api/rpc/*` server functions.

**Database:** schema is created inline at startup in `frontend::db::initialize_schema`. No migrations framework yet. All tests use `sqlite::memory:` for isolation.

**Server URL (mobile):** hardcoded to `http://127.0.0.1:3000` in `mobile/src/main.rs` via `use_context_provider`. Will become a first-launch setup screen.

### shared/src/

```
lib.rs              — Settings, ValueResponse, LibraryContents, LibrarySection
```

### frontend/src/

```
lib.rs              — Route, App, styles, ScreenLayout (feature-gated)
data.rs             — Feature-gated data transport (mobile=reqwest, web/server=rpc)
rpc.rs              — #[get]/#[post] server functions (mounted by dioxus::server::router)
db.rs               — pool init, schema, queries (feature = "server")
scanner.rs          — library directory scanning (feature = "server")
pages/{landing,settings}.rs
components/{top_nav,bottom_nav}.rs  — feature = web / mobile respectively
```

### server/src/

```
main.rs             — dioxus::launch (WASM) / dioxus::serve (native)
lib.rs              — re-exports backend under `server` feature for tests
backend.rs          — /api/* REST router (mobile-facing) + integration tests
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
cargo test -p omnibus-frontend --features server            # db + scanner tests
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

See [ROADMAP.md](ROADMAP.md) for planned OPDS feed, epub/m4a scanning, Dioxus Native mobile app, and the full intended database schema.
