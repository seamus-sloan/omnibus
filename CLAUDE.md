# CLAUDE.md

Guidance for Claude Code when working in this repo. This file is an index — detailed rules and recipes live in [.claude/](.claude/).

Omnibus is a self-hosted ebook/audiobook library (see [docs/roadmap/0-0-summary.md](docs/roadmap/0-0-summary.md)). The current counter app is a placeholder.

## Rules

Numbered rules in [.claude/rules/](.claude/rules/), applied in order. Follow them mechanically.

- [01-dev-environment.md](.claude/rules/01-dev-environment.md) — always work inside `nix develop`; env vars the shellHook sets.
- [02-error-handling.md](.claude/rules/02-error-handling.md) — `thiserror` for predictable failures, `anyhow` for unpredictable ones and handlers.
- [03-unit-testing.md](.claude/rules/03-unit-testing.md) — sibling `<mod>/tests.rs`, `test_support` per crate, happy + per-variant coverage.
- [04-playwright.md](.claude/rules/04-playwright.md) — full E2E conventions (selectors, fixtures, `expectMutation`, error paths).
- [05-rust-style.md](.claude/rules/05-rust-style.md) — Rust style guide: comments, function/file shape, errors, tests, mechanics. Long-form rationale in [docs/style-guide.md](docs/style-guide.md).
- [98-keep-skills-fresh.md](.claude/rules/98-keep-skills-fresh.md) — update skills when the code they reference changes.
- [99-end-of-session.md](.claude/rules/99-end-of-session.md) — end-of-session checklist (docs sync, fmt/clippy, coverage, line-count cap).

**Line-count cap:** every file in `CLAUDE.md` / `.claude/` stays under ~200 lines. Split by topic when it grows past that — enforced by rule 99.

## Skills

Auto-discoverable skills in [.claude/skills/](.claude/skills/) — Claude Code loads each `SKILL.md` automatically, and each is invokable via `/<name>` (e.g. `/ui-validate`).

- [add-backend-route](.claude/skills/add-backend-route/SKILL.md) — adding an Axum page or API endpoint end-to-end.
- [add-playwright-flow](.claude/skills/add-playwright-flow/SKILL.md) — adding a new E2E spec.
- [ui-validate](.claude/skills/ui-validate/SKILL.md) — bring up a port-walking dev server, log in as the seeded admin, drive the browser, poll `/api/_health` for rebuild signal.

### Browser tools

When you need to drive the running web app to verify a UI change, use **[`ui-validate`](.claude/skills/ui-validate/SKILL.md)**. It uses **Claude Preview** (`mcp__Claude_Preview__preview_*`) — each agent gets its own isolated headless Chromium, so parallel agents across workspaces never share a browser.

Do not use Chrome DevTools MCP or Claude in Chrome for routine agent verification — both share a single browser instance (one at a time), so they break down with parallel agents. Use `qa-browser` only when you explicitly want to watch the agent work in your real browser for a one-off session.

## Architecture

Five-crate Cargo workspace: `shared/` (serde types), `db/` (data layer + indexer), `frontend/` (Dioxus UI + server functions), `server/` (fullstack binary + REST router), `mobile/` (thin native shell).

Full crate descriptions, per-crate module maps, request flow diagrams, and mobile-auth details live in [.claude/architecture.md](.claude/architecture.md).

## Quick commands

```bash
# Multiplexed dev stack (server + ios + android + playwright panes)
just serve                                                  # Zellij
just serve-pc                                               # process-compose

# Idempotent dev-server bring-up — port-walks from $PORT (default 3000)
# up to PORT+20, reuses an existing omnibus server if found, seeds the
# admin user from $OMNIBUS_DEV_SEED_USER, writes .claude/runtime/{port,env.sh}.
# Used by the `ui-validate` skill; safe to re-run.
just dev-up
source .claude/runtime/env.sh                               # picks up OMNIBUS_PORT + PLAYWRIGHT_BASE_URL

# Fullstack dev (serves SSR + WASM hydration at http://localhost:8080 by default)
dx serve --platform web -p omnibus

# Server only (native backend, no WASM bundle)
cargo run -p omnibus                                        # start at http://0.0.0.0:3000
cargo test -p omnibus                                       # /api/* REST integration tests
cargo test -p omnibus-db                                    # db + ebook + scanner tests
cargo clippy                                                # lint default-members crates
cargo fmt                                                   # format all crates

# Playwright E2E (server must be running; baseURL = $PLAYWRIGHT_BASE_URL or :3000)
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
