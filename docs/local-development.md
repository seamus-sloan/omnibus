# Local development

Everything you need to build, run, and test Omnibus from source. If you only
want to *run* a server, use [Docker](docker.md) instead — this page is for
working on the code.

Omnibus is a Rust workspace ([Axum](https://github.com/tokio-rs/axum) +
[Dioxus](https://dioxuslabs.com/)) over SQLite, plus a native SwiftUI iOS app
and an Android WebView shell.

## Prerequisites

[Nix](https://wiki.nixos.org/wiki/NixOS_Wiki) pins every dependency — the Rust
toolchain, SQLite, Node, mobile SDKs. **Always work inside the dev shell.**

```bash
nix develop                 # slim default shell (spawns bash)
nix develop --command zsh   # …keeping your shell
direnv allow                # or auto-load via direnv (once)
```

macOS extras, only if you build the iOS app: Xcode with at least one iOS
Simulator installed, and `jq` on `PATH`.

## Running the app

**The `just` recipes are the quickest way in.** They self-wrap in the right Nix
shell, so they run straight from the slim `default` shell:

```bash
just serve       # full stack in Zellij — server + web + iOS + Android + Playwright panes
just serve-pc    # …the same stack via process-compose
just dev-up      # just the web server: port-walks, seeds the admin user
just dev-status  # what's running, on which port
just dev-bounce  # cleanly restart a wedged dev server (e.g. after a migration)
just dev-down    # stop it
```

Prefer to drive the pieces yourself?

```bash
dx serve --platform web -p omnibus   # fullstack SSR + WASM at http://localhost:8080
cargo run -p omnibus                 # server only (native backend) at http://0.0.0.0:3000
```

Every worktree gets its own 10-port window, so two checkouts can serve at once
without colliding. Both launchers publish the port they picked to
`.claude/runtime/env.sh` — `source` it rather than assuming `:3000`.

## Nix shells

The flake exposes purpose-built shells so daily cargo work doesn't pull in
Playwright + mobile + audit tooling. Opt into a heavier shell only when you need
it (the `just` recipes above pick the right one for you):

| Shell | When to use |
|---|---|
| `default` | Daily `cargo` / `clippy` / `test` / editor — what direnv auto-loads |
| `.#web` | `dx serve --platform web`, `just dev-up`, anything that bundles WASM |
| `.#e2e` | `pnpm exec playwright test` (the Chromium bundle lives here) |
| `.#mobile` | Android builds (Rust cross-targets + JDK + Android NDK detect) — the native iOS app needs no Nix shell |
| `.#audit` | `cargo audit` / `cargo deny` |

## Configuration

The shell sets non-secret defaults:

| Variable | Default |
|---|---|
| `PORT` | `3000` (per-worktree window base) |
| `DATABASE_URL` | `sqlite://omnibus.db?mode=rwc` |
| `CARGO_TARGET_DIR` | `~/.cache/cargo-target/<worktree>` |

Secret-bearing config lives in a gitignored `.env` — copy
[`.env.example`](../.env.example) on first checkout. It's the canonical,
annotated reference for every supported variable (library paths, metadata
provider keys, SMTP, storage overrides).

## Test fixtures

The public-domain EPUB/audiobook fixtures aren't in git — they live as a GitHub
release asset so Nix's tree-copy stays small:

```bash
just fixtures    # one-time download; instant no-op thereafter
```

`just test` runs it for you; Playwright's `globalSetup` and the public-domain db
tests fail with a pointer to it when they're missing.

## Tests & lint

```bash
just test    # full matrix: db + server + frontend(--features server) + shared
just lint    # cargo fmt --check + clippy across the crate/feature matrix + stylelint
just check   # lint then test

just coverage       # lcov + per-file summary via cargo-llvm-cov over nextest
just coverage-html  # browsable per-line report
```

Per-crate, when you want a tighter loop. Note that `cargo test --workspace`
**skips** the frontend rpc/page tests (they need `--features server` to compile
the server-function bodies) and `mobile` (out of default-members, no tests), so
run each crate explicitly:

```bash
cargo test -p omnibus                              # /api/* REST integration tests
cargo test -p omnibus-db                           # db + scanner + sync tests
cargo test -p omnibus-frontend --features server   # rpc + page tests
cargo test -p omnibus-shared                       # shared serde / ebook / progress tests
cargo test -p omnibus-mcp                          # MCP tool layer (out of default-members)
```

### Web E2E (Playwright)

The server must be running; Chromium comes from Nix. The project uses **pnpm**
(not npm) and TypeScript 7, with Biome as linter/formatter:

```bash
cd ui_tests/playwright
pnpm install                # first time only; do NOT run `pnpm exec playwright install`
pnpm exec playwright test
just lint-ts                # biome check + tsc --noEmit
```

## Mobile apps

The two mobile surfaces are separate apps built in different ways: Android is
the Dioxus shell in the `mobile/` crate, iOS is a native SwiftUI project under
`omnibus-ios/`. Both talk to a running Omnibus server, so **start the server
first** (`just serve` handles this).

In both multiplexers only the server starts on its own — the `android`, `ios`,
and `playwright` panes are preloaded but idle (press Enter in the Zellij pane,
or select it and press F7 in process-compose).

### iOS — native SwiftUI app (`omnibus-ios/`)

An Xcode project (`omnibus-ios/omnibus.xcodeproj`, scheme `omnibus`) rather than
a Cargo crate — it speaks the same `/api/*` REST surface, and `cargo` never
builds it. Its recipes drive system `xcodebuild` / `simctl`, so none of them
enter a Nix shell:

```bash
just ios-sim      # boot the newest iPhone simulator, build, install, launch
just ios-build    # compile check against a generic simulator destination
just ios-test     # omnibusTests unit suite — the invocation CI runs
just ios-test-ui  # omnibusUITests
```

The app has no baked-in server URL — `just ios-sim` prints this workspace's dev
server URL to enter on the Connect screen. Pin a specific simulator with
`OMNIBUS_IOS_SIM_UDID` if you don't want the newest iPhone runtime.

### Android — Dioxus shell (`mobile/` crate)

A thin native shell hosting the system WebView over the shared Dioxus frontend,
pointed at `http://127.0.0.1:3000`. The `android` pane runs
`dx serve --platform android --package omnibus-mobile` inside the `.#mobile`
shell; start it yourself with the same command once the emulator is up. One-time
setup:

1. **Install Android Studio** — [developer.android.com/studio](https://developer.android.com/studio)
2. **Install the NDK** — **Tools → SDK Manager → SDK Tools → check "NDK (Side by side)" → Apply**
3. **Create an emulator** — **Tools → Device Manager → Create Virtual Device** (API 33+ recommended), then start it.
4. **Enter the mobile dev shell** — it auto-detects `ANDROID_HOME` / `ANDROID_NDK_HOME`:

   ```bash
   nix develop .#mobile --command zsh
   echo $ANDROID_HOME       # e.g. /Users/<you>/Library/Android/sdk
   echo $ANDROID_NDK_HOME   # e.g. …/sdk/ndk/28.x.x
   ```

   If they're empty, set them manually:

   ```bash
   export ANDROID_HOME=$HOME/Library/Android/sdk
   export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$(ls $ANDROID_HOME/ndk | tail -1)
   ```

5. If the app can't reach the server after boot, run `adb reverse tcp:3000 tcp:3000`.

## Project layout

Six-crate Cargo workspace:

| Crate | Responsibility |
|---|---|
| `shared/` | serde types shared across crates |
| `db/` | data layer, indexer, migrations |
| `frontend/` | Dioxus UI + server functions |
| `server/` | fullstack binary + REST router |
| `mobile/` | thin native Android shell over the system WebView |
| `mcp/` | MCP tool layer over the `/api/*` surface (out of default-members) |

Alongside it, `omnibus-ios/` holds the native SwiftUI iOS client — an Xcode
project, not a Cargo crate, so `cargo` / `just lint` / `just test` never touch
it. `site/` is the marketing site published to GitHub Pages (static HTML, no
build step).

## Conventions

Deeper architecture — module maps, request-flow diagrams, mobile auth — is in
[architecture.md](architecture.md). The rules contributors follow live under
[.claude/rules/](../.claude/rules/):

| Topic | Rule |
|---|---|
| Dev environment, env vars | [01-dev-environment.md](../.claude/rules/01-dev-environment.md) |
| Error handling | [02-error-handling.md](../.claude/rules/02-error-handling.md) |
| Unit & integration testing | [03-unit-testing.md](../.claude/rules/03-unit-testing.md) |
| Playwright E2E conventions | [04-playwright.md](../.claude/rules/04-playwright.md) |
| Rust style | [05-rust-style.md](../.claude/rules/05-rust-style.md) (rationale in [style-guide.md](style-guide.md)) |
| SQL migrations | [06-migrations.md](../.claude/rules/06-migrations.md) |
| SSR/WASM hydration parity | [07-hydration.md](../.claude/rules/07-hydration.md) |
| Offline write queueing | [08-offline-writes.md](../.claude/rules/08-offline-writes.md) |
| Content validators / ETags | [09-content-validators.md](../.claude/rules/09-content-validators.md) |

UI work also has a design system: [design/atrium-design-system.md](design/atrium-design-system.md).
