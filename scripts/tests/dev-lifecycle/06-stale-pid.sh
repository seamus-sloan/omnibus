#!/usr/bin/env bash
# Scenario 6: `.claude/runtime/server.pid` points to a dead PID; nothing
# is bound to the recorded port. `dev-status` should detect "no server"
# (exit 1, not exit 2 — wedged would imply a live process), and `dev-down`
# should silently clean the stale files.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
RUNTIME_DIR=".claude/runtime"
SCENARIO="06-stale-pid"

# Stash any existing runtime state so we don't fight a real running server.
STASH=""
if [ -d "$RUNTIME_DIR" ]; then
    STASH="$RUNTIME_DIR.testbackup.$$"
    mv "$RUNTIME_DIR" "$STASH"
fi
restore() {
    rm -rf "$RUNTIME_DIR"
    if [ -n "$STASH" ] && [ -d "$STASH" ]; then
        mv "$STASH" "$RUNTIME_DIR"
    fi
}
trap restore EXIT

mkdir -p "$RUNTIME_DIR"

# Find a PID that's guaranteed dead: pick a high integer and confirm it
# isn't running. 99999 isn't reserved on macOS/Linux and is well above
# what's typically allocated.
dead_pid=99999
while kill -0 "$dead_pid" 2>/dev/null; do
    dead_pid=$((dead_pid + 1))
    [ "$dead_pid" -gt 200000 ] && { echo "FAIL: $SCENARIO — couldn't find a dead PID"; exit 1; }
done
echo "$dead_pid" >"$RUNTIME_DIR/server.pid"
# Record a port too so dev-down knows where to look (it'll probe and get
# nothing, which is fine — the PID is dead so it short-circuits to clean).
echo "3999" >"$RUNTIME_DIR/port"

# dev-status: should exit 1 ("no server"), not 2 ("wedged"). Stale PID
# means no live process, so the "wedged" branch should be skipped.
set +e
out="$(scripts/dev-server-status.sh 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 1 ]; then
    echo "FAIL: $SCENARIO — dev-status expected exit 1 (no server), got $rc"
    echo "      output: $out"
    exit 1
fi
echo "ok: dev-status exited 1 for stale-PID state"

# dev-down: should clean the stale files and exit 0 without trying to
# kill anything (nothing to kill).
echo "$dead_pid" >"$RUNTIME_DIR/server.pid"  # re-write in case dev-status cleaned
echo "3999" >"$RUNTIME_DIR/port"
set +e
out="$(scripts/dev-server-down.sh 2>&1)"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then
    echo "FAIL: $SCENARIO — dev-down expected exit 0, got $rc"
    echo "      output: $out"
    exit 1
fi
if [ -f "$RUNTIME_DIR/server.pid" ]; then
    echo "FAIL: $SCENARIO — dev-down did not clean stale server.pid"
    exit 1
fi
echo "ok: dev-down cleaned stale runtime files"

echo "PASS: $SCENARIO"
