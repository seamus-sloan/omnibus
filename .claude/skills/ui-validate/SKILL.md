---
name: ui-validate
description: End-to-end recipe for validating omnibus UI changes in a real browser — brings up a port-walking dev server, logs in as the seeded admin, polls /api/_health for the rebuild signal, and verifies via snapshot/screenshot. Triggers when you need to drive the running web app to verify a change, when another agent may already own :3000, when the page isn't reloading after an edit, or when login state is uncertain.
---

# Validate a UI change in the browser

This is the canonical flow for validating any change to the Dioxus web UI. Uses **Claude Preview** (`mcp__Claude_Preview__preview_*`) — each agent gets its own isolated headless Chromium, so parallel agents across workspaces never share a browser. It assumes nothing about the current server state — safe to re-run.

## 1. Bring the server up (idempotent)

```bash
just dev-up
```

What this does (`scripts/dev-server-up.sh`):

- Probes `GET /api/_health` starting at `$PORT` (default 3000), walking up to `PORT+9` (the per-workspace window).
- Reuses an existing omnibus server on a probed port **only when its `repo_root` matches this workspace** — sibling `jj` workspaces' servers are skipped, not silently shared.
- Fails fast with a remediation line if every port in range is held by a foreign process.
- Starts `dx serve --platform web --fullstack --port <chosen>` in the background (daemonized so it survives the agent shell); output goes to `.claude/runtime/server.log`, PID to `.claude/runtime/server.pid`.
- Requires `OMNIBUS_DEV_SEED_USER` (sourced from `.env`). If unset, prints `"copy .env.example to .env and re-enter nix develop"` and exits 1.
- Writes `.claude/runtime/port` and `.claude/runtime/env.sh`.
- Emits one parseable line on stdout: `OMNIBUS_DEV_READY port=<n> repo_root=<path> build_id=<n> action=<reuse|started> pid=<n>`.

If a server is already up for this workspace, `just dev-up` returns in under 2s with `action=reuse`. If it's not, the first run takes ~30-60s while `dx serve` builds. **Don't kill and restart** a healthy server between sessions — the long-running model is intentional. Use `just dev-bounce` after sqlx migrations or when a `dx serve` build wedges.

If it exits 1 telling you to set `OMNIBUS_DEV_SEED_USER`:

```bash
cp .env.example .env   # then re-enter nix develop, or `set -a; source .env; set +a`
just dev-up
```

## 2. Load the runtime env

```bash
source .claude/runtime/env.sh
```

Exports `OMNIBUS_PORT` and `PLAYWRIGHT_BASE_URL`. **Never hardcode the port** — your workspace's stable port (3000/3010/3020/3030 by workspace name; see `flake.nix`) might be overridden if a foreign process held it.

## 3. Capture the current build id

```bash
BEFORE_BUILD=$(curl -s "http://127.0.0.1:$OMNIBUS_PORT/api/_health" | jq -r .build_id)
```

This is the process-start timestamp. Any Rust HMR cycle restarts the process, so the id changes — that's the signal to know a rebuild actually landed.

## 4. Open the page and log in

Use the Chrome DevTools MCP. The session cookie is `HttpOnly` (see `server/src/auth/handlers.rs`) and can't be injected from JS — drive the login form:

1. `mcp__Claude_Preview__preview_start` → `http://localhost:$OMNIBUS_PORT/login`
2. `mcp__Claude_Preview__preview_fill` username `admin` / password `omnibus-dev` (matches `.env.example`)
3. `mcp__Claude_Preview__preview_click` the Sign in button
4. `mcp__Claude_Preview__preview_snapshot` to confirm the redirect to the landing page

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

Then reload via `mcp__Claude_Preview__preview_eval` with `location.reload()`.

**Frontend-only changes** (Dioxus components, CSS) may not restart the server, so `build_id` won't move within 30s. In that case skip the build-id poll, reload directly, and rely on DOM testid presence (`take_snapshot`) to detect the new render.

## 6. Verify

- `mcp__Claude_Preview__preview_snapshot` — content/structure assertion (DOM tree).
- `mcp__Claude_Preview__preview_screenshot` — visual proof to share with the user.
- `mcp__Claude_Preview__preview_console_logs` — JS error checks.
- `mcp__Claude_Preview__preview_network` — failed-fetch checks.
- `mcp__Claude_Preview__preview_eval` — CSS / computed-style checks, e.g. `getComputedStyle(document.querySelector('.foo')).color`.

## 7. Run Playwright against the same server

```bash
source .claude/runtime/env.sh   # if not already sourced
cd ui_tests/playwright && npx playwright test
```

`PLAYWRIGHT_BASE_URL` makes the suite hit the walked-up port — both `baseURL` and the `Origin` header are derived from it, so the CSRF `origin_check` middleware stays happy.

## Common pitfalls

- **Snapshot shows the login form.** Cookie expired or cleared. Repeat step 4.
- **`build_id` never changes after an edit.** The change was frontend-only and didn't trigger a server restart. Skip the build-id poll, reload directly.
- **`dev-up` exits 1 with "ports … all held by non-omnibus processes."** Run `lsof -iTCP:$PORT-$((PORT+9)) -sTCP:LISTEN -P` to see what's holding them. Most likely cause: a previous `dx serve` you forgot about — `cat .claude/runtime/server.pid` and `just dev-down` (identity-checked), or pick a different starting `PORT`.
- **`dev-up` exits 2 with "server unhealthy — run `just dev-bounce`."** `dx serve` is up but its most recent rebuild errored. Read `.claude/runtime/server.log` for the compile error, fix it, then `just dev-bounce`.
- **`origin_check` 403s a POST.** `OMNIBUS_PUBLIC_ORIGIN` is out of sync with the actual port. The dev-up script sets this; running `dx serve` by hand bypasses it. Use `just dev-up` instead.
- **You see `port N held by sibling workspace ...` on stderr.** Expected when another `jj` workspace's omnibus server is bound to your starting port. `dev-up` walks past it; nothing to fix.
