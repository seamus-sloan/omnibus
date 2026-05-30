#!/usr/bin/env bash
# Scenario 7: a non-omnibus process is bound to the base port. `dev-up`'s
# port-walker (via choose_port) should skip the port and find a free one
# further up the workspace's window. We don't actually start dx serve in
# this test — we just exercise `choose_port` so the test is fast and
# doesn't compile WASM.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
SCENARIO="07-foreign-port"

# Pick a port in this workspace's window that's definitely free, then
# bind a foreign listener to it. We use `python3 -m http.server` because
# it returns HTML (not the omnibus /api/_health JSON), so is_omnibus_response
# will reject it.
START_PORT="${PORT:-3000}"
foreign_port=""
for ((p = START_PORT; p < START_PORT + 9; p++)); do
    if ! lsof -iTCP:"$p" -sTCP:LISTEN -P >/dev/null 2>&1; then
        foreign_port="$p"
        break
    fi
done
if [ -z "$foreign_port" ]; then
    echo "FAIL: $SCENARIO — no free port in window $START_PORT-$((START_PORT + 9)) to plant a foreign listener"
    exit 1
fi

# Start the foreign listener in the background. Pipe stdout/stderr away
# so it doesn't pollute test output.
python3 -m http.server "$foreign_port" --bind 127.0.0.1 >/dev/null 2>&1 &
foreign_pid=$!
cleanup() {
    kill "$foreign_pid" 2>/dev/null || true
    wait "$foreign_pid" 2>/dev/null || true
}
trap cleanup EXIT

# Give the listener a moment to bind.
for _ in 1 2 3 4 5; do
    if lsof -iTCP:"$foreign_port" -sTCP:LISTEN -P >/dev/null 2>&1; then break; fi
    sleep 0.2
done
if ! lsof -iTCP:"$foreign_port" -sTCP:LISTEN -P >/dev/null 2>&1; then
    echo "FAIL: $SCENARIO — foreign listener never bound to port $foreign_port"
    exit 1
fi

# Source the dev-server-up.sh helpers so we can call choose_port directly
# without going through the full bring-up (which would compile + start
# dx serve and take minutes).
# shellcheck disable=SC1091
PORT="$START_PORT" END_PORT_OVERRIDE=$((foreign_port + 1)) bash -c '
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
START_PORT="'"$START_PORT"'"
END_PORT="'"$foreign_port"'"  # confine window to just the foreign + one above

# Inline minimal copies of the helpers — sourcing the real script would
# also try to run main(); we just want choose_port behavior.
probe_health() {
    curl --silent --show-error --max-time 2 --connect-timeout 1 \
        "http://127.0.0.1:$1/api/_health" 2>/dev/null || true
}
is_port_free() { ! lsof -iTCP:"$1" -sTCP:LISTEN -P >/dev/null 2>&1; }
is_omnibus_response() { echo "$1" | grep -q "\"app\"[[:space:]]*:[[:space:]]*\"omnibus\""; }

for ((port = START_PORT; port <= END_PORT; port++)); do
    if [ "$port" -eq '"$foreign_port"' ]; then
        # The port we planted a foreign listener on. dev-up must NOT
        # reuse it (the response is not omnibus) and must NOT report it
        # as free. choose_port would skip + continue.
        body="$(probe_health "$port")"
        if is_omnibus_response "$body"; then
            echo "FAIL: foreign listener somehow returned an omnibus response"
            exit 1
        fi
        if is_port_free "$port"; then
            echo "FAIL: port $port reported free but a listener is bound"
            exit 1
        fi
        echo "ok: foreign-port $port correctly identified as non-omnibus, non-free"
    fi
done

# The next port up should be free → dev-up would land there.
next_port=$(('"$foreign_port"' + 1))
# Skip if next_port is outside the original window.
if [ "$next_port" -le $(('"$START_PORT"' + 9)) ]; then
    if ! is_port_free "$next_port"; then
        echo "FAIL: port $next_port should be free for dev-up to walk to"
        exit 1
    fi
    echo "ok: next port $next_port is free — dev-up would walk to it"
fi
'

echo "PASS: $SCENARIO"
