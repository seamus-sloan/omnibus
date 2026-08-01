# CLAUDE.md

Guidance for Claude Code when working in this repo. This file is an index — detailed rules and recipes live in [.claude/](.claude/).

Omnibus is a self-hosted ebook/audiobook library. Foundations and browse/discovery have shipped; reading/listening is in progress — the UI is a real library app (landing grid + table, EPUB reader, audiobook player, command-palette search, auth, author/series/tag discovery).

## Rules

Numbered rules in [.claude/rules/](.claude/rules/), applied in order. Follow them mechanically.

- [01-dev-environment.md](.claude/rules/01-dev-environment.md) — always work inside `nix develop`; env vars the shellHook sets.
- [02-error-handling.md](.claude/rules/02-error-handling.md) — `thiserror` for predictable failures, `anyhow` for unpredictable ones and handlers.
- [03-unit-testing.md](.claude/rules/03-unit-testing.md) — sibling `<mod>/tests.rs`, `test_support` per crate, happy + per-variant coverage.
- [04-playwright.md](.claude/rules/04-playwright.md) — full E2E conventions (selectors, fixtures, `expectMutation`, error paths).
- [05-rust-style.md](.claude/rules/05-rust-style.md) — Rust style guide: comments, function/file shape, errors, tests, mechanics. Long-form rationale in [docs/style-guide.md](docs/style-guide.md).
- [06-migrations.md](.claude/rules/06-migrations.md) — authoring SQL migrations: `NNNN_` naming, never-edit-applied, the `_norm` backfill pattern, testing against `sqlite::memory:`, and the dev-bounce step.
- [07-hydration.md](.claude/rules/07-hydration.md) — SSR/WASM hydration parity: never feature-gate a component body on `web`; how to confirm and fix a hydration mismatch.
- [08-offline-writes.md](.claude/rules/08-offline-writes.md) — what the mutation outbox may queue: content state only, never configuration or commands; the four tests, and what each excludes.
- [09-content-validators.md](.claude/rules/09-content-validators.md) — the two content validators (response `ETag` vs wire etag), why they're derived rather than stored, and the `conditional::apply` path every byte-serving endpoint takes.
- [98-keep-skills-fresh.md](.claude/rules/98-keep-skills-fresh.md) — update skills when the code they reference changes.
- [99-end-of-session.md](.claude/rules/99-end-of-session.md) — end-of-session checklist (docs sync, fmt/clippy, coverage, line-count cap).

**Line-count cap:** every file in `CLAUDE.md`, `AGENTS.md`, and `.claude/` stays under ~200 lines. Split by topic when it grows past that — enforced by rule 99.

## Skills

Auto-discoverable skills in [.claude/skills/](.claude/skills/) — Claude Code loads each `SKILL.md` automatically, and each is invokable via `/<name>` (e.g. `/ui-validate`).

- [add-backend-route](.claude/skills/add-backend-route/SKILL.md) — adding an Axum page or API endpoint end-to-end.
- [add-playwright-flow](.claude/skills/add-playwright-flow/SKILL.md) — adding a new E2E spec.
- [ui-validate](.claude/skills/ui-validate/SKILL.md) — bring up a port-walking dev server, log in as the seeded admin, drive the browser, poll `/api/_health` for rebuild signal.

### Browser tools

When you need to drive the running web app to verify a UI change, use **[`ui-validate`](.claude/skills/ui-validate/SKILL.md)**. It uses **Claude Preview** (`mcp__Claude_Preview__preview_*`) — each agent gets its own isolated headless Chromium, so parallel agents across workspaces never share a browser.

Do not use Chrome DevTools MCP or Claude in Chrome for routine agent verification — both share a single browser instance (one at a time), so they break down with parallel agents. Use `qa-browser` only when you explicitly want to watch the agent work in your real browser for a one-off session.

## Architecture

Five-crate Cargo workspace: `shared/` (serde types), `db/` (data layer + indexer), `frontend/` (Dioxus UI + server functions), `server/` (fullstack binary + REST router), `mobile/` (thin native shell, Android-only).

Alongside it, `omnibus-ios/` is a native SwiftUI client — an Xcode project, not a Cargo crate; `just ios-build` / `ios-test` / `ios-test-ui` / `ios-sim` wrap xcodebuild + simctl for it (no cargo target touches it). It is the iOS surface; `mobile/` is the Android shell (nothing builds the `mobile/` crate for iOS anymore). It speaks the same `/api/*` REST surface.

Full crate descriptions, per-crate module maps, request flow diagrams, and mobile-auth details live in [docs/architecture.md](docs/architecture.md).

## Version control

This repo is developed with [Jujutsu (`jj`)](https://jj-vcs.github.io/jj/) over the Git backend, and changes land as GitHub pull requests — one PR per change, conventional-commit titles (`feat:` / `fix:` / `chore:` / `docs:`).

- **Never amend or rewrite an already-pushed commit.** `jj` auto-snapshots the working copy into the current change on every command, so editing files while `@` sits on a pushed bookmark silently rewrites published history into a force-push. Run `jj new <bookmark>` to start a fresh change *before* editing — even when the working copy is clean.
- Routine flow: `jj git fetch` to sync; `jj bookmark create <name>` for a branch; `jj describe -m "…"` to set the message; `jj bookmark move <name> --to @` then `jj git push`.
- The dev tooling assumes `jj` — `scripts/dev-server-up.sh` identity-checks the workspace root so sibling `jj` workspaces don't collide on a port.

Operator-specific conventions (ticket prefixes, standing workspace names) live in personal config, not here.

## Quick commands

```bash
# Multiplexed dev stack (server + native ios + android + playwright panes)
just serve                                                  # Zellij
just serve-pc                                               # process-compose

# Idempotent dev-server bring-up — port-walks from $PORT (default 3000)
# up to PORT+9, reuses an existing omnibus server if found, seeds the
# admin user from $OMNIBUS_DEV_SEED_USER, writes .claude/runtime/{port,env.sh}.
# Used by the `ui-validate` skill; safe to re-run.
just dev-up
source .claude/runtime/env.sh                               # picks up OMNIBUS_PORT + PLAYWRIGHT_BASE_URL

# Fullstack dev (serves SSR + WASM hydration at http://localhost:8080 by default)
dx serve --platform web -p omnibus

# Server only (native backend, no WASM bundle)
cargo run -p omnibus                                        # start at http://0.0.0.0:3000

# Tests & lint — aggregate targets cover the full crate matrix in one go
just test                                                   # db + server + frontend(server + mobile features) + shared
just lint                                                   # cargo fmt --check + clippy -D warnings (incl. mobile + frontend-server + frontend-web wasm32) + stylelint
just lint-css                                               # structural CSS lint only (stylelint; catches unclosed rules in frontend/assets)
just lint-ts                                                # Playwright TS: biome check + tsc --noEmit (TypeScript 7), in .#e2e
just check                                                  # lint then test
# …or per-crate (note: `cargo test --workspace` SKIPS frontend rpc/page tests
#  and mobile — the rpc/page tests need --features server to compile the
#  server-function bodies; mobile is out of default-members and has no tests):
cargo test -p omnibus                                       # /api/* REST integration tests
cargo test -p omnibus-db                                    # db + scanner + sync tests
cargo test -p omnibus-frontend --features server            # rpc + page tests (server feature required)
cargo test -p omnibus-shared                                # shared serde / ebook / progress tests
cargo clippy                                                # lint default-members (server, shared, frontend)
cargo fmt                                                   # format all crates

# Playwright E2E (server must be running; baseURL = $PLAYWRIGHT_BASE_URL or :3000)
# The Playwright project uses pnpm (not npm) and TypeScript 7; Biome is its
# linter/formatter. `just lint-ts` runs biome check + tsc --noEmit.
cd ui_tests/playwright && pnpm install                      # first time
cd ui_tests/playwright && pnpm exec playwright test         # run all
just lint-ts                                                # biome + typecheck (TS project)

# Native iOS app (omnibus-ios/; system xcodebuild + simctl, no nix shell)
just ios-build                                              # compile check (generic simulator destination)
just ios-test                                               # omnibusTests unit suite — same script CI runs
just ios-test-ui                                            # omnibusUITests
just ios-sim                                                # boot newest iPhone sim, build, install, launch

# Hybrid mobile shell (mobile/ crate — Android surface)
cargo build -p omnibus-mobile
dx serve --platform android --package omnibus-mobile
adb reverse tcp:3000 tcp:3000                               # after Android emulator boots
```

## Project direction

See the [roadmap project board](https://github.com/users/seamus-sloan/projects/2/views/9) for the phased plan (foundations, browse/discovery, reading/listening, personalization, device sync, admin, mobile).
