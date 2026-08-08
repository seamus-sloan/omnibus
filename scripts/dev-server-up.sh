#!/usr/bin/env bash
# Idempotent dev-server bring-up for Claude's ui-validate workflow and for
# anyone who wants a port-walking, "just give me a server" front door.
#
# What it does:
#   1. Probes 127.0.0.1:<port> across this worktree's window ($PORT..PORT+9;
#      flake.nix gives each worktree its own base). Reuses an existing
#      omnibus instance only when its repo_root is ours; picks the first
#      free port otherwise; exits 1 if every candidate is held.
#   2. Verifies OMNIBUS_DEV_SEED_USER is set (otherwise the server boots
#      without a known admin login and `ui-validate` will fail at the
#      `/login` step). Tells the user to copy .env.example -> .env.
#   3. Starts `dx serve --platform web --fullstack --port <chosen> -p omnibus`
#      in the background. Output -> .claude/runtime/server.log, PID ->
#      .claude/runtime/server.pid. Polls /api/_health until 200 (90s cap).
#   4. Writes:
#        .claude/runtime/port      -> "<chosen>"
#        .claude/runtime/env.sh    -> exports OMNIBUS_PORT + PLAYWRIGHT_BASE_URL
#      So Claude and humans can `source .claude/runtime/env.sh` before
#      running `pnpm exec playwright test` or driving the preview.
#
# Re-running this script is safe — if /api/_health already says "omnibus"
# at the chosen port, we just reuse and re-emit the runtime files.

set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
# The port walk, health probes, and runtime-file writer are shared with
# scripts/dev-serve-fg.sh so the two front doors can't hand one port to two
# worktrees. MY_REPO_ROOT, RUNTIME_DIR, START_PORT and END_PORT come from here.
# shellcheck source=scripts/lib/dev-port.sh
source scripts/lib/dev-port.sh

mkdir -p "$RUNTIME_DIR"

# HMR-tolerant probe. `dx serve` briefly stops serving while it rebuilds
# the backend binary after a Rust change; /api/_health then returns
# connection-refused or 5xx for a few seconds before the new build is
# back up. This helper retries up to ~5s before declaring the server
# down so callers don't false-positive during a hot reload.
probe_health_with_retry() {
    local port="$1"
    local deadline=$(($(date +%s) + 5))
    local body
    while [ "$(date +%s)" -le "$deadline" ]; do
        body="$(probe_health "$port")"
        if is_omnibus_response "$body"; then
            echo "$body"
            return 0
        fi
        sleep 1
    done
    echo "$body"
    return 1
}

# The old flat 90s covered a warm rebuild but not a freshly-created
# worktree, which compiles the whole workspace into an empty
# CARGO_TARGET_DIR — and per-worktree ports make fresh worktrees routine,
# so the cold build can't be the case that fails. The budget is generous
# instead, and the common failure (a compile error, which makes dx exit)
# is caught by watching the child rather than by waiting the clock out.
wait_for_health() {
    local port="$1" server_pid="$2"
    local deadline=$(($(date +%s) + ${OMNIBUS_DEV_BUILD_TIMEOUT_SECS:-900}))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        local body
        body="$(probe_health "$port")"
        if is_omnibus_response "$body"; then
            echo "$body"
            return 0
        fi
        # dx is gone — a compile error or a crash. Fail now; the caller's
        # remediation line points at the log that says why.
        if ! kill -0 "$server_pid" 2>/dev/null; then
            return 1
        fi
        sleep 1
    done
    return 1
}

start_server() {
    local port="$1"

    # Without a seed user the freshly-booted server has no admin login and
    # the ui-validate skill can't proceed past /login. Fail fast with a
    # clear remediation rather than booting into an unusable state.
    if [ -z "${OMNIBUS_DEV_SEED_USER:-}" ]; then
        cat >&2 <<'EOF'
dev-up: OMNIBUS_DEV_SEED_USER is not set.
  -> copy .env.example to .env and re-enter `nix develop` (or `source .env`).
     The default value in .env.example is admin:omnibus-dev.
EOF
        exit 1
    fi

    # Export overrides for the child server. PORT and OMNIBUS_PUBLIC_ORIGIN
    # need to match the chosen port so origin_check accepts cookie POSTs
    # from the browser. Allow both `localhost` (humans typing into a
    # browser) and `127.0.0.1` (Playwright's PLAYWRIGHT_BASE_URL, which
    # this same script writes a few lines below) — without the IP form,
    # the dx serve --fullstack proxy rewrites Host: away from the
    # Origin: localhost the spec sent, and every cookie-authed POST 403s.
    # OMNIBUS_DEV_SEED_USER is inherited from the dev shell (.env via
    # flake.nix shellHook).
    export PORT="$port"
    export OMNIBUS_PUBLIC_ORIGIN="http://localhost:$port,http://127.0.0.1:$port"

    echo "dev-up: starting dx serve on port $port (log: $RUNTIME_DIR/server.log)" >&2
    # Three things together detach `dx serve` from this script's lifetime
    # so the server survives the agent / shell that invoked `just dev-up`:
    #   * nohup     — traps SIGHUP so terminal disconnect can't kill it.
    #   * `&`       — backgrounds; the script exits without waiting.
    #   * disown    — removes the job from this shell's job table so the
    #                 shell can't SIGHUP it on exit (bash builtin, portable).
    # setsid would be cleaner but isn't on macOS by default.
    nohup dx serve --platform web --fullstack --port "$port" --package omnibus \
        >"$RUNTIME_DIR/server.log" 2>&1 &
    local server_pid=$!
    disown "$server_pid" 2>/dev/null || disown 2>/dev/null || true
    echo "$server_pid" >"$RUNTIME_DIR/server.pid"

    if ! wait_for_health "$port" "$server_pid" >/dev/null; then
        # Kill the orphan rather than leaving it bound to $port — a stale,
        # unhealthy process would block the next dev-up (or get "reused"
        # as if it were healthy if it manages to start serving later).
        if kill -0 "$server_pid" 2>/dev/null; then
            echo "dev-up: server did not become healthy within ${OMNIBUS_DEV_BUILD_TIMEOUT_SECS:-900}s; killing pid $server_pid" >&2
            kill "$server_pid" 2>/dev/null || true
            # Reap dx-wrap's grandchildren too (dx spawns the actual server
            # in a subprocess). pkill -P walks one level; that's enough
            # here because dx is the only child of $server_pid we care
            # about.
            pkill -P "$server_pid" 2>/dev/null || true
        fi
        rm -f "$RUNTIME_DIR/server.pid"
        echo "dev-up: tail the log for the underlying error:" >&2
        echo "  tail -n 50 $RUNTIME_DIR/server.log" >&2
        exit 1
    fi
}

main() {
    local choice port mode
    if ! choice="$(choose_port)"; then
        echo "dev-up: ports $START_PORT-$END_PORT are all held by non-omnibus processes." >&2
        echo "        Stop one of them or set PORT to a different starting point." >&2
        exit 1
    fi
    port="${choice%%:*}"
    mode="${choice##*:}"

    local action
    if [ "$mode" = "reuse" ]; then
        echo "dev-up: reusing existing omnibus server on port $port" >&2
        action="reuse"
    else
        start_server "$port"
        action="started"
    fi

    write_runtime_files "$port"

    local body build_id pid
    body="$(probe_health "$port")"
    build_id="$(echo "$body" | sed -nE 's/.*"build_id"[[:space:]]*:[[:space:]]*"([0-9]+)".*/\1/p')"
    pid="$(cat "$RUNTIME_DIR/server.pid" 2>/dev/null || echo '')"

    # Human-readable line to stderr; the agent-parseable marker to stdout.
    # Anyone driving this script programmatically should `grep '^OMNIBUS_DEV_READY '`
    # against stdout — the format is stable and the only line on stdout.
    echo "omnibus dev server ready at http://127.0.0.1:$port (build_id=$build_id); log in as ${OMNIBUS_DEV_SEED_USER:-<unset>}" >&2
    echo "OMNIBUS_DEV_READY port=$port repo_root=$MY_REPO_ROOT build_id=$build_id action=$action pid=$pid"
}

main "$@"
