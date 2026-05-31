#!/usr/bin/env bash
# Peek at this workspace's dev server. Non-mutating.
#
# Exits:
#   0 — server up and healthy. Emits OMNIBUS_DEV_READY marker on stdout.
#   1 — no server running for this workspace.
#   2 — server is up on this workspace's port range but unhealthy / wedged
#       (probably a `dx serve` rebuild failure). Run `just dev-bounce` to
#       restart cleanly.
#
# Output contract:
#   stdout: at most one line —
#     OMNIBUS_DEV_READY port=<n> repo_root=<abs-path> build_id=<n> action=reuse pid=<n>
#   stderr: human-readable diagnostics.

set -euo pipefail

MY_REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$MY_REPO_ROOT"

RUNTIME_DIR=".claude/runtime"
START_PORT="${PORT:-3000}"
END_PORT=$((START_PORT + 9))

probe_health() {
    local port="$1"
    curl --silent --show-error --max-time 2 --connect-timeout 1 \
        "http://127.0.0.1:${port}/api/_health" 2>/dev/null || true
}

is_omnibus_response() {
    echo "$1" | grep -q '"app"[[:space:]]*:[[:space:]]*"omnibus"'
}

extract_field() {
    # $1 = body, $2 = field name; emits the value (string).
    echo "$1" | sed -nE "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"([^\"]*)\".*/\\1/p"
}

# Walk the workspace's port window looking for a healthy server whose
# repo_root matches. HMR-tolerant: retries each port for up to ~5s on
# transient probe failures.
probe_with_retry() {
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

# 1. If a PID file exists, prefer that port (faster path; avoids walking).
recorded_port=""
recorded_pid=""
if [ -f "$RUNTIME_DIR/port" ]; then
    recorded_port="$(cat "$RUNTIME_DIR/port" 2>/dev/null || true)"
fi
if [ -f "$RUNTIME_DIR/server.pid" ]; then
    recorded_pid="$(cat "$RUNTIME_DIR/server.pid" 2>/dev/null || true)"
fi

check_port() {
    local port="$1"
    local body server_root
    body="$(probe_with_retry "$port")" || return 1
    server_root="$(extract_field "$body" "repo_root")"
    [ -n "$server_root" ] && [ "$server_root" = "$MY_REPO_ROOT" ] || return 1
    # Healthy + mine.
    local build_id
    build_id="$(extract_field "$body" "build_id")"
    local pid="$recorded_pid"
    if [ -z "$pid" ] && [ -n "$recorded_port" ] && [ "$recorded_port" = "$port" ]; then
        pid=""
    fi
    echo "OMNIBUS_DEV_READY port=$port repo_root=$MY_REPO_ROOT build_id=$build_id action=reuse pid=$pid"
    return 0
}

if [ -n "$recorded_port" ]; then
    if check_port "$recorded_port"; then
        exit 0
    fi
    # Recorded port didn't answer. If the PID is alive, the server is
    # wedged (process running but not responding to /api/_health) — that's
    # a different signal than "nothing there".
    if [ -n "$recorded_pid" ] && kill -0 "$recorded_pid" 2>/dev/null; then
        echo "dev-status: PID $recorded_pid alive on port $recorded_port but /api/_health is failing — likely a wedged dx serve build. Run \`just dev-bounce\` to restart cleanly." >&2
        exit 2
    fi
fi

# Walk the window in case the recorded port is stale.
for ((port = START_PORT; port <= END_PORT; port++)); do
    if check_port "$port"; then
        exit 0
    fi
done

echo "dev-status: no omnibus server for $MY_REPO_ROOT in ports $START_PORT-$END_PORT. Start one with \`just dev-up\`." >&2
exit 1
