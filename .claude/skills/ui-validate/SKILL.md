---
name: ui-validate
description: End-to-end recipe for validating omnibus UI changes in a real browser preview — brings up a port-walking dev server, logs in as the seeded admin, polls /api/_health for rebuild signal, and verifies via snapshot/screenshot. Triggers when you need to drive the running web app to verify a change (preview_*), when another agent may already own :3000, when the page isn't reloading after an edit, or when login state is uncertain.
---

# Validate a UI change in the browser preview

This is the canonical flow for validating any change to the Dioxus web UI. It assumes nothing about the current server state — it's safe to re-run.

## 1. Bring the server up (idempotent)

```bash
just dev-up
```

What this does (`scripts/dev-server-up.sh`):

- Probes `GET /api/_health` starting at `$PORT` (default 3000), walking up to `PORT+20`.
- Reuses an existing omnibus server on a probed port, or picks the first free port.
- Fails fast with a remediation line if every port in range is held by a foreign process.
- Starts `dx serve --platform web --fullstack --port <chosen>` in the background; output goes to `.claude/runtime/server.log`, PID to `.claude/runtime/server.pid`.
- Requires `OMNIBUS_DEV_SEED_USER` (sourced from `.env`). If unset, prints `"copy .env.example to .env and re-enter nix develop"` and exits 1.
- Writes `.claude/runtime/port` and `.claude/runtime/env.sh`.

If it exits 1 telling you to set `OMNIBUS_DEV_SEED_USER`:

```bash
cp .env.example .env   # then re-enter nix develop, or `set -a; source .env; set +a`
just dev-up
```

## 2. Load the runtime env

```bash
source .claude/runtime/env.sh
```

Exports `OMNIBUS_PORT` and `PLAYWRIGHT_BASE_URL`. **Never hardcode 3000** — the server may be on 3001+ if another agent claimed 3000.

## 3. Capture the current build id

```bash
BEFORE_BUILD=$(curl -s "http://127.0.0.1:$OMNIBUS_PORT/api/_health" | jq -r .build_id)
```

This is the process-start timestamp. Any Rust HMR cycle restarts the process, so the id changes — that's the signal to know a rebuild actually landed.

## 4. Start the preview and log in

```
preview_start http://localhost:$OMNIBUS_PORT
```

The session cookie is `HttpOnly` (see `server/src/auth/handlers.rs:118`) and can't be injected from JS — so you must drive the login form:

1. `preview_eval`: `location.href = "/login"`
2. `preview_fill` username `admin` / password `omnibus-dev` (matches `.env.example`)
3. `preview_click` the Sign in button
4. `preview_snapshot` to confirm the redirect to the landing page

Cache this state for the rest of the session. Only redo the login if a later snapshot shows the login form again (cookie expired or got cleared).

## 5. Edit code, then wait for the rebuild

After every code edit that affects the server:

```bash
# Poll until build_id changes, max 30s
for i in $(seq 1 30); do
  NOW_BUILD=$(curl -s "http://127.0.0.1:$OMNIBUS_PORT/api/_health" | jq -r .build_id)
  [ "$NOW_BUILD" != "$BEFORE_BUILD" ] && break
  sleep 1
done
BEFORE_BUILD="$NOW_BUILD"
```

Then `preview_eval`: `location.reload()`.

**Frontend-only changes** (Dioxus components, CSS) may not restart the server, so `build_id` won't move within 30s. In that case fall back to a single `preview_eval: location.reload()` and rely on DOM testid presence (`preview_snapshot`) to detect the new render.

## 6. Verify

- `preview_snapshot` — content/structure assertion
- `preview_screenshot` — visual proof to share with the user
- `preview_console_logs` / `preview_network` — error checks
- `preview_inspect` — CSS value checks

## 7. Run Playwright against the same server

```bash
source .claude/runtime/env.sh   # if not already sourced
cd ui_tests/playwright && npx playwright test
```

`PLAYWRIGHT_BASE_URL` makes the suite hit the walked-up port — both `baseURL` and the `Origin` header are derived from it, so the CSRF `origin_check` middleware stays happy.

## Common pitfalls

- **Snapshot shows the login form.** Cookie expired or cleared. Repeat step 4.
- **`build_id` never changes after an edit.** The change was frontend-only and didn't trigger a server restart. Skip the build-id poll, reload directly.
- **`dev-up` exits 1 with "ports … all held by non-omnibus processes."** Run `lsof -iTCP:3000-3020 -sTCP:LISTEN -P` to see what's holding them. Most likely cause: a previous `dx serve` you forgot about — `cat .claude/runtime/server.pid` and `kill` it, or pick a different starting `PORT`.
- **`origin_check` 403s a POST.** `OMNIBUS_PUBLIC_ORIGIN` is out of sync with the actual port. The dev-up script sets this; running `dx serve` by hand bypasses it. Use `just dev-up` instead.
