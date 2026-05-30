# Dev-server lifecycle test matrix

Scenario-by-scenario tests for the `just dev-{up,status,down,bounce}` CLI
introduced by the dev-server-coordination work. The cheap scenarios are
automated as numbered shell scripts in this directory; the expensive ones
(those that need a real `dx serve` compile, ~minutes per run) are documented
as manual recipes below.

Run the automated ones from the repo root, e.g. `bash scripts/tests/dev-lifecycle/06-stale-pid.sh`.
Each script:
- starts from a clean `.claude/runtime/` (saves + restores any existing files).
- exits 0 on pass and prints `PASS: <scenario>`, exits non-zero with `FAIL: <reason>` on failure.
- needs no `dx serve` build (otherwise it's in the manual list below).

## Automated (in this directory)

| # | Script | Scenario |
|---|---|---|
| 6 | `06-stale-pid.sh` | `.claude/runtime/server.pid` points to a dead PID; nothing bound. `dev-up` should clean and treat as cold start; `dev-status` should report no server. |
| 7 | `07-foreign-port.sh` | A non-omnibus process bound to `$PORT`. `dev-up` should walk to the next port; stderr names the foreign occupant. |

## Manual (require real `dx serve` + minutes per run)

These verify behavior that depends on an actual running server. Documented
here as reproducer recipes; run them when you change the lifecycle scripts
in a meaningful way. None of these require special tooling — just two
terminals and the dev shell.

### 1. Working state (already-up reuse is a fast no-op)
```bash
# Terminal A:
just dev-up                            # cold start; takes a minute
just dev-up | grep OMNIBUS_DEV_READY   # expect action=reuse, returns in <2s
```
PASS when the second `dev-up` returns within ~2s with `action=reuse`.

### 2. Cold start (no server, no PID file)
```bash
just dev-down                          # ensure clean
just dev-up                            # expect action=started; emits marker
```
PASS when stdout has exactly one `OMNIBUS_DEV_READY … action=started` line
and the server responds to `curl http://127.0.0.1:$PORT/api/_health`.

### 3. Slow startup (server boots slowly)
Apply a one-line test patch:
```rust
// server/src/main.rs, top of the launched body
tokio::time::sleep(std::time::Duration::from_secs(15)).await;
```
Then:
```bash
just dev-down
just dev-up                            # should poll patiently within the 90s budget
```
PASS when `dev-up` does NOT exit early with "did not become healthy" and
eventually emits `action=started`. Revert the patch when done.

### 4. HMR in progress
```bash
# With a server already up (action=reuse), trigger a rebuild:
touch server/src/main.rs
just dev-status                        # immediately after touch
```
PASS when `dev-status` exits 0 with a NEW `build_id` (different from the
pre-touch one), proving `probe_with_retry` waited out the brief rebuild.

### 5. Wedged build
Introduce a deliberate compile error in `server/src/main.rs` (e.g. add
`let x: i32 = "broken";` somewhere). With a server already up, `dx serve`
will try to rebuild on the next file save and fail; the previous binary
keeps running, but if you SIGTERM it the rebuilt one won't come up. To
simulate: kill the running server (`kill $(cat .claude/runtime/server.pid)`)
then:
```bash
just dev-status                        # expect exit 2: server unhealthy
just dev-up                            # expect exit 2 + "run \`just dev-bounce\`"
```
PASS when `dev-up` refuses to auto-restart. Fix the compile error and
`just dev-bounce` to recover.

### 8. Sibling-workspace collision
Requires two `jj` workspaces (e.g. `omnibus` + `omnibus-xray`).
```bash
# Workspace A (~/Repos/omnibus): start a server on the default port 3000.
cd ~/Repos/omnibus && just dev-up

# Workspace B (~/Repos/omnibus-xray): force probing port 3000.
cd ~/Repos/omnibus-xray
PORT=3000 scripts/dev-server-up.sh    # should skip 3000 + walk to 3001
```
PASS when B's stderr contains `port 3000 held by sibling workspace …, skipping`
and stdout's `OMNIBUS_DEV_READY` has `port=3001` (or another in B's window).
Also: `cd ~/Repos/omnibus-xray && just dev-down` must refuse to kill A's
server (the identity check rejects the mismatched `repo_root`).

### 9. Migration drift (stretch — not implemented)
Reserved for a future change: extend `/api/_health` with a
`migrations_applied` count, have `dev-status` compare to on-disk
`db/migrations/*.sql` and warn when they drift.
