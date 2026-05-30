#!/usr/bin/env bash
# Stop THIS workspace's dev server. Identity-checked: refuses to kill a
# PID whose /api/_health reports a different repo_root (i.e. a sibling
# `jj` workspace's server bound to a port we wrote to our PID file by
# accident, or anything else not ours).
#
# Exits:
#   0 — stopped (or already stopped). Emits OMNIBUS_DEV_READY action=stopped
#       on stdout when an actual kill happened.
#   2 — PID file points at a server whose repo_root != ours. Refused.

set -euo pipefail

MY_REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$MY_REPO_ROOT"

RUNTIME_DIR=".claude/runtime"

if [ ! -f "$RUNTIME_DIR/server.pid" ]; then
    echo "dev-down: no server.pid; nothing to stop." >&2
    exit 0
fi

pid="$(cat "$RUNTIME_DIR/server.pid" 2>/dev/null || true)"
port="$(cat "$RUNTIME_DIR/port" 2>/dev/null || true)"

if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
    echo "dev-down: PID ${pid:-<empty>} is not running; cleaning stale runtime files." >&2
    rm -f "$RUNTIME_DIR/server.pid" "$RUNTIME_DIR/port" "$RUNTIME_DIR/env.sh"
    exit 0
fi

# Identity check via /api/_health.repo_root. If the server doesn't
# respond, we have a wedged process — still ours by PID file, kill it.
# If it responds with a different repo_root, REFUSE — that's a sibling
# workspace's server and we must not touch it.
body=""
if [ -n "$port" ]; then
    body="$(curl --silent --show-error --max-time 2 --connect-timeout 1 \
        "http://127.0.0.1:${port}/api/_health" 2>/dev/null || true)"
fi
server_root="$(echo "$body" | sed -nE 's/.*"repo_root"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/p')"
if [ -n "$server_root" ] && [ "$server_root" != "$MY_REPO_ROOT" ]; then
    echo "dev-down: PID $pid on port $port serves $server_root, not $MY_REPO_ROOT — refusing to kill a sibling workspace's server." >&2
    exit 2
fi

# Either ours by repo_root match, or wedged (no response) but recorded
# in our PID file — either way, kill it.
echo "dev-down: stopping pid $pid (port ${port:-?})" >&2
kill "$pid" 2>/dev/null || true
# Reap the dx serve grandchild (dx wraps the actual server in a subprocess).
pkill -P "$pid" 2>/dev/null || true

# Give it a moment to actually exit before we clean the runtime files.
for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
done
if kill -0 "$pid" 2>/dev/null; then
    echo "dev-down: pid $pid didn't exit after SIGTERM; sending SIGKILL." >&2
    kill -9 "$pid" 2>/dev/null || true
    pkill -9 -P "$pid" 2>/dev/null || true
fi

rm -f "$RUNTIME_DIR/server.pid" "$RUNTIME_DIR/port" "$RUNTIME_DIR/env.sh"

echo "OMNIBUS_DEV_READY port=${port:-} repo_root=$MY_REPO_ROOT action=stopped"
