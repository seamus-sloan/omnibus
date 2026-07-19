# Mobile E2E — Maestro

E2E tests for the Omnibus **mobile** app (`omnibus-mobile`). Web E2E lives
separately under [`../playwright/`](../playwright/).

## Why Maestro (and not Playwright)

The mobile crate is a **hybrid app**: a thin native shell (`mobile/src/main.rs`)
hosting a system WebView (WKWebView on iOS, Android System WebView) that renders
the shared `omnibus-frontend` markup with `features = ["mobile"]`. Maestro has
first-class WebView support, drives iOS + Android from one YAML flow on a
simulator/emulator, and matches the visible-text selector model the web suite
already uses. Playwright can only reach the Android WebView (iOS WKWebView
speaks the WebKit inspector protocol, not CDP), so it isn't a single
cross-platform track.

Because the mobile UI *is* the web frontend, most component logic is already
covered transitively by Playwright + the frontend unit tests. These flows
target the **mobile-only** surface: the Connect screen, auth/token persistence
across cold start, and the WebView reader.

## Running

Maestro is Nix-provided in the `.#mobile` shell (flake `mobileExtras`) — no
manual install, and don't `curl | bash` the upstream installer.

Prereqs: a booted simulator with the app installed (`dx serve --platform ios
--package omnibus-mobile` handles build + install + launch), and a running
server for the happy-path flows (`just dev-up`).

```bash
just e2e-mobile                        # whole suite; picks up dev-up's port
just e2e-mobile --include-tags smoke   # extra args pass through to maestro
OMNIBUS_PORT=3000 just e2e-mobile      # explicit port beats env.sh
```

Or from the `just serve` multiplexer: the **maestro** tab (zellij) / process
(process-compose) runs the suite once per activation, pinned to the server
tab's :3000. Raw invocation, if you need it:

```bash
nix develop .#mobile --command maestro test -e SERVER_URL=http://127.0.0.1:3000 ui_tests/maestro/flows/
```

`maestro studio` (same shell) opens an interactive inspector against the
running app — useful for discovering how a WebView element is addressed
before writing an assertion.

## CI

[`mobile-e2e.yml`](../../.github/workflows/mobile-e2e.yml) runs the same suite
on both platforms:

- **iOS** — macOS runner: native server + `dx build --platform ios`, boots a
  simulator, flows target `http://127.0.0.1:3000` (sim shares the host
  network).
- **Android** — KVM-accelerated Linux runner: `dx build --target
  x86_64-linux-android` (emulator images are x86_64; the default arm64 APK
  would crash on ABI mismatch), `reactivecircus/android-emulator-runner`
  boots the emulator, flows target `http://10.0.2.2:3000` (the emulator's
  alias for the host loopback — this is why flows parameterize `SERVER_URL`).

**Triggers (#1034):** the iOS job runs on `push` to `main` and on PRs, both
path-filtered to mobile-affecting code (macOS minutes are expensive) —
`server/**`/`db/**` included because the flows probe the real server's
`/api/_health`. The Android job is manual-only behind the `android`
`workflow_dispatch` input until its flows are green on the emulator (#1035).

**Flake armor:** on top of each flow's `retry` block (Maestro hard-caps
`maxRetries` at 3), CI retries the whole `maestro test` invocation up to 3
times — the residual failure modes (XCUITest driver startup timeout, the
WebView process dying under CI load, dropped `inputText` characters) are
environmental and recover on a fresh invocation. A pass on attempt > 1 emits
a warning annotation and keeps the Maestro debug artifact so the flake stays
visible. Per-flow results land in the job's step summary (JUnit).

**Stress runs:** the `stress` dispatch input fans the iOS job into N identical
matrix instances (e.g. `[1,2,3,4,5]`) for flake-hunting; run once normally
first so the stress instances start from warm caches.

## State model — hermetic flows, serial execution

- `maestro test <dir>` runs flows **serially on one device, alphabetically**.
  There is no parallelism to design around.
- App state (the persisted server URL, bearer token) is real on-disk app data
  and **persists across flows** — nothing auto-resets between them.
- Every flow therefore starts with `launchApp: { clearState: true }`, which
  wipes the app's data container: each flow is hermetic and order-independent.
  `connect_error.yaml` doubles as the regression test for this — it runs right
  after `connect.yaml` persisted a URL, and asserts the first-run Connect
  screen still appears.
- Shared setup (e.g. "connect to the dev server") belongs in a reusable
  subflow invoked via `runFlow` — Maestro's fixture equivalent — not in
  reliance on a previous flow's side effects.

## Selector conventions

Mirror the Playwright rules ([`04-playwright.md`](../../.claude/rules/04-playwright.md)):
prefer visible text (labels, buttons, placeholders). HTML `id`/`data-testid`
attributes do **not** surface as iOS accessibility identifiers through the
WebView, so text is the primary contract — e.g. the URL field is addressed by
its placeholder. `hideKeyboard` is unreliable on iOS; lay out screens so
submit controls stay reachable with the keyboard up, and tap them directly.

## Environment variables

Flows read `${SERVER_URL}` (default `http://127.0.0.1:3000`, overridden via
`maestro test -e SERVER_URL=...` — the `just e2e-mobile` recipe wires this
from `$OMNIBUS_PORT` / `.claude/runtime/env.sh`).
