#!/usr/bin/env bash
# Browser driver for the agentic-exploration swarm: one playwright-repl server
# per agent, each with its own session, port, and browser process.
#
# Usage:
#   driver.sh up <agent-count>     start N servers, print the port manifest
#   driver.sh run <n> <command>    send one command to agent n, print the result
#   driver.sh status               show which agents are up
#   driver.sh down                 stop every server
#
# Why one server per agent rather than one shared browser:
#   Run r-20260828-01 died because three subagents shared a single browser tab.
#   They shared a cookie jar, so "three different users" collapsed into one and
#   two agents correctly aborted rather than journal entries under a wrong
#   actor. A server per agent gives a browser per agent, which makes that class
#   of failure impossible rather than merely unlikely.
#
# Why not playwright-repl's own MCP mode:
#   N MCP clients against one server land back in a shared browser. The HTTP
#   surface, one server per port, is what keeps them apart.
#
# Why a private browser directory:
#   playwright-repl bundles Playwright 1.62.1 (Chromium build 1234) while the
#   flake pins ~1.59 (build 1217) for the E2E suite. They cannot share
#   PLAYWRIGHT_BROWSERS_PATH, and running `playwright install` inside
#   ui_tests/playwright would diverge the suite from the flake — see
#   .claude/rules/04-playwright.md.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
DRIVER="$HERE/driver"
PORT_BASE="${OMNIBUS_EXPLORE_PORT_BASE:-9223}"
STATE="$ROOT/.claude/runtime/explore/driver"
MANIFEST="$STATE/ports.json"

driver::ensure_deps() {
  export PLAYWRIGHT_BROWSERS_PATH="$DRIVER/browsers"
  if [ ! -x "$DRIVER/node_modules/.bin/playwright-repl" ]; then
    echo "installing driver deps (first run)…" >&2
    (cd "$DRIVER" && npm install --no-audit --no-fund >/dev/null 2>&1)
  fi
  if [ ! -d "$DRIVER/browsers" ]; then
    echo "downloading the driver's own chromium (kept out of the flake)…" >&2
    (cd "$DRIVER" && ./node_modules/.bin/playwright install chromium >/dev/null 2>&1)
  fi
}

driver::port() { echo $((PORT_BASE + $1 - 1)); }

driver::alive() {
  # 000 means nothing accepted the connection; any HTTP code means a server is
  # there. `/` is deliberately not a route, so 404 is the healthy answer.
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://127.0.0.1:$1/" 2>/dev/null)" || true
  [ -n "$code" ] && [ "$code" != "000" ]
}

cmd="${1:?usage: driver.sh up <n> | run <n> <command> | status | down}"

case "$cmd" in
  up)
    count="${2:?usage: driver.sh up <agent-count>}"
    [[ "$count" =~ ^[0-9]+$ ]] && [ "$count" -ge 1 ] \
      || { echo "agent-count must be a positive integer" >&2; exit 2; }
    driver::ensure_deps
    mkdir -p "$STATE"
    entries=""
    for i in $(seq 1 "$count"); do
      port="$(driver::port "$i")"
      if driver::alive "$port"; then
        echo "  agent-$i: reusing server on $port" >&2
      else
        (cd "$DRIVER" && PLAYWRIGHT_BROWSERS_PATH="$DRIVER/browsers" \
          nohup ./node_modules/.bin/playwright-repl --silent --http \
            --http-port "$port" -s "agent-$i" \
            >"$STATE/agent-$i.log" 2>&1 &)
        for _ in $(seq 1 40); do driver::alive "$port" && break; sleep 1; done
        driver::alive "$port" \
          || { echo "agent-$i failed to start on $port — see $STATE/agent-$i.log" >&2; exit 1; }
        echo "  agent-$i: started on $port" >&2
      fi
      entries="$entries{\"actor\":\"agent-$i\",\"port\":$port},"
    done
    printf '[%s]\n' "${entries%,}" | tee "$MANIFEST"
    ;;

  run)
    n="${2:?usage: driver.sh run <n> <command>}"
    shift 2
    port="$(driver::port "$n")"
    driver::alive "$port" || { echo "agent-$n has no server on $port — run driver.sh up first" >&2; exit 1; }
    body="$(python3 -c 'import json,sys; print(json.dumps({"command": sys.argv[1]}))' "$*")"
    curl -sS --max-time 180 -X POST "http://127.0.0.1:$port/run" \
      -H 'Content-Type: application/json' -d "$body"
    echo
    ;;

  status)
    [ -f "$MANIFEST" ] || { echo "no manifest — nothing started"; exit 0; }
    while read -r actor port; do
      if driver::alive "$port"; then state=up; else state=DOWN; fi
      printf '  %-9s port %s  %s\n' "$actor" "$port" "$state"
    done < <(python3 -c 'import json,sys
for e in json.load(open(sys.argv[1])): print(e["actor"], e["port"])' "$MANIFEST")
    ;;

  down)
    stopped=0
    for port in $(seq "$PORT_BASE" $((PORT_BASE + 31))); do
      pid="$(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)"
      [ -n "$pid" ] && { kill $pid 2>/dev/null || true; stopped=$((stopped + 1)); }
    done
    rm -f "$MANIFEST"
    echo "stopped $stopped server(s)"
    ;;

  *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
