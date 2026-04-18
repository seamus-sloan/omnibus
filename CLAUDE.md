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

Auto-discoverable how-tos in [.claude/skills/](.claude/skills/) — use them when their description matches the task.

- [add-backend-route.md](.claude/skills/add-backend-route.md) — adding an Axum page or API endpoint end-to-end.
- [add-playwright-flow.md](.claude/skills/add-playwright-flow.md) — adding a new E2E spec.
- [jj-basics.md](.claude/skills/jj-basics.md) — fetch / new / describe / bookmark / push.
- [jj-workspaces.md](.claude/skills/jj-workspaces.md) — parallel agent work on one repo.
- [jj-advanced.md](.claude/skills/jj-advanced.md) — squash / rebase / abandon / undo / op log / conflicts.

## Architecture

Cargo workspace with two crates:

- **`server/`** (`omnibus`) — Axum SSR server. Dioxus is used only as a templating engine; interactivity is plain JavaScript.
- **`mobile/`** (`omnibus-mobile`) — Dioxus Native mobile app. Communicates with the server via JSON API.

**Server request flow:** Axum handler → `db/` query → Dioxus SSR component renders HTML string → `Html(...)` response. JSON API routes skip SSR and return `Json(...)` directly.

**Mobile data flow:** Dioxus signal/effect → `reqwest` call to `/api/*` → signal update → re-render.

**Database:** schema is created inline at startup in `db::initialize_schema`. No migrations framework yet. All tests use `sqlite::memory:` for isolation.

**Server URL (mobile):** hardcoded to `http://127.0.0.1:3000` in `mobile/src/main.rs` via `use_context_provider`. Will become a first-launch setup screen.

### server/src/

```
main.rs
lib.rs
backend.rs          — Axum router + AppState + handlers
db.rs               — pool init, schema, queries
frontend/
  mod.rs            — Route enum, App component, render_document, styles, SSR tests
  pages/{landing,settings}.rs
  components/nav.rs
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
main.rs             — dioxus::launch, Route enum, App, ServerUrl context, CSS
pages/{landing,settings}.rs
components/nav.rs
```

## Quick commands

```bash
# Multiplexed dev stack (server + ios + android + playwright panes)
just serve                                                  # Zellij
just serve-pc                                               # process-compose

# Server only
cargo run -p omnibus                                        # start at http://0.0.0.0:3000
cargo test -p omnibus                                       # all server tests
cargo test -p omnibus <test_name>                           # single test
cargo clippy                                                # lint all crates
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
