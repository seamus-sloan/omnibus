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
dx serve --port 3000 --package omnibus                      # run server with hot-reload, devserver fixed at port 3000
cargo clippy                                                # lint all crates
cargo fmt                                                   # format all crates

# E2E UI tests (TypeScript Playwright, against a running local server)
cd ui_tests/playwright && npm install && npx playwright install --with-deps chromium   # first-time setup
cd ui_tests/playwright && npm test                          # run all E2E tests
cd ui_tests/playwright && npm run test:ui                   # interactive UI mode

# Mobile
cargo build -p omnibus-mobile                               # build mobile app
xcrun simctl boot "iPhone 17" 2>/dev/null; dx serve --platform ios --package omnibus-mobile   # run in iOS Simulator (requires Xcode)
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
```

### ui_tests/

```
playwright/         — TypeScript Playwright E2E tests for the server UI
  package.json
  playwright.config.ts
  tsconfig.json
  tests/
    flows/          — one *.spec.ts per user flow (counter, settings, …)
    utils/          — cross-flow helpers (nav, api mutation assertions)
    fixtures/       — extended `test` / `expect` exports; hook point for shared state
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
- **E2E tests:** TypeScript Playwright tests in `ui_tests/playwright/`, run with `npm test` from that directory against a locally running server. Cover user-facing flows on the web UI. Follow the conventions below.
- **Mobile E2E:** not yet implemented. The mobile crate is Dioxus Native (not a WebView), so Playwright cannot reach it. When added, this will be a separate track under `ui_tests/` (likely Appium + WebdriverIO, or Maestro) and will require stable accessibility ids on interactive elements in `mobile/src/`.

### Playwright E2E conventions

These rules exist so every flow is tested the same way; don't diverge without updating this section first.

**Style — functional helpers + fixtures, never page-object classes.** Import `test` and `expect` from `tests/fixtures/test.ts` (not directly from `@playwright/test`) so shared fixtures apply uniformly. Factor reusable selectors and actions into plain functions, not classes.

**Selectors — semantic first, `locator()` last, never XPath.** Use this preference order:

1. `page.getByRole(...)` — buttons, headings, links, form landmarks, live regions (`status`, `alert`). Also use for form labels: `getByRole("button", { name: "Save" })`.
2. `page.getByText(...)` — visible text that isn't tied to a role.
3. `page.getByLabel(...)` — form inputs with a `<label for=...>`. Add a proper label in the SSR markup rather than reaching for a test id.
4. `page.getByTestId(...)` — only when no role / text / label fits. Add `"data-testid": "..."` (alongside the existing `id`) to the Dioxus rsx markup. Keep the testid name stable and meaningful — it's part of the UI contract.
5. `page.locator(...)` — last resort, only for things nothing else can express.

Never use XPath. If you find yourself wanting XPath, the SSR markup probably needs a role, label, or testid added.

**Structure — one file per flow under `tests/flows/`.** Each flow file contains:

1. **One layout test** (`renders the <page> layout`) asserting the destination page's structure: key elements visible, shared nav present (via `expectNavVisible` from `utils/nav.ts`). No user actions in the layout test.
2. **One or more action tests**, one per user action, covering happy path and error path. Action tests drive the UI, assert network contracts, then assert UI state.

Flow-specific helpers (e.g. `fillSettingsForm`) live inside the flow's spec file. Only cross-flow helpers go to `utils/`.

**Waits — `expect.poll` and Playwright auto-waiting only. No `waitForTimeout`.** If the DOM is going to change, poll for it. If a request must complete before asserting, `await` the response via `expectMutation` from `utils/api.ts`.

**Network — every mutating request (POST/PUT/PATCH/DELETE) must be asserted.** Wrap the user action that triggers a mutation in `expectMutation(page, { method, url, expectedBody, expectedStatus }, action)`. It arms `waitForRequest`/`waitForResponse`, runs the action, checks the payload and status, and returns the request/response for further assertions — crucially ensuring the test waited for the response before any subsequent UI assertion. Reads (GET) are not asserted unless the assertion depends on their data.

**Error paths — force failures with `page.route`.** For error-path tests, intercept the mutating route and `route.fulfill({ status: 500, ... })` before triggering the action, then still use `expectMutation` to verify the request fired with the expected payload and observed the forced status. Assert the UI surfaces the error (status text, error class, unchanged state, etc).

**Example shape** (see `tests/flows/settings.spec.ts` for the full version):

```ts
test("saves library paths and shows a success status", async ({ page }) => {
  await page.goto("/settings");
  await page.getByLabel("Ebook Library Path").fill(path);

  await expectMutation(
    page,
    { method: "POST", url: "/api/settings", expectedBody: { ... }, expectedStatus: 200 },
    async () => page.getByRole("button", { name: "Save" }).click(),
  );

  await expect(page.getByRole("status")).toHaveText("Settings saved.");
});
```

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
