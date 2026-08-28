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
#   two agents correctly aborted rather than write journal entries under a
#   wrong actor. A server per agent gives a browser per agent, which makes that
#   class of failure impossible rather than merely unlikely.
#
# Why not playwright-repl's own MCP mode:
#   N MCP clients against one server land back in a shared browser. The HTTP
#   surface, one server per port, is what keeps them apart.
#
# Why it shares the flake's Chromium:
#   playwright-repl declares `playwright: ^1.59.1`, which npm would float to
#   whatever is newest — a build the flake's bundle does not contain. The
#   driver's package.json pins that transitive dependency to the same version
#   as the flake's playwright-driver.browsers, so one Nix Chromium serves the
#   E2E suite and the driver alike and nothing ever downloads a browser. Bump
#   the two together; see .claude/rules/04-playwright.md.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
DRIVER="$HERE/driver"
PORT_BASE="${OMNIBUS_EXPLORE_PORT_BASE:-9223}"
STATE="$ROOT/.claude/runtime/explore/driver"
MANIFEST="$STATE/ports.json"

driver::ensure_deps() {
  # The browser comes from the flake, so the driver must run inside a shell
  # that provides it. Failing here beats a confusing "Executable doesn't
  # exist" from deep inside Playwright.
  if [ -z "${PLAYWRIGHT_BROWSERS_PATH-}" ]; then
    echo "PLAYWRIGHT_BROWSERS_PATH is unset — run inside 'nix develop .#e2e'" >&2
    echo "(or via scripts/with-dev-env.sh e2e …), which pins the Chromium bundle." >&2
    return 1
  fi
  if [ ! -x "$DRIVER/node_modules/.bin/playwright-repl" ]; then
    echo "installing driver deps (first run)…" >&2
    # `npm ci` from the committed lockfile: reproducible across machines, and
    # it cannot rewrite package-lock.json, so `driver.sh up` never dirties the
    # worktree. The pinned playwright version is the whole point — an install
    # free to resolve differently would silently stop sharing the flake's
    # Chromium.
    (cd "$DRIVER" && npm ci --no-audit --no-fund >/dev/null 2>&1)
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
        (cd "$DRIVER" && \
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
    n="${2:?usage: driver.sh run <n> \"<command>\"}"
    [[ "$n" =~ ^[0-9]+$ ]] && [ "$n" -ge 1 ] \
      || { echo "agent number must be a positive integer (got: $n)" >&2; exit 2; }
    shift 2
    # Exactly one argument, so an unquoted command cannot be silently
    # reassembled with collapsed whitespace — which would change the JS being
    # evaluated without anyone noticing.
    [ "$#" -eq 1 ] || {
      echo "expected exactly one command argument — quote it: driver.sh run $n \"<command>\"" >&2
      exit 2
    }
    port="$(driver::port "$n")"
    driver::alive "$port" || { echo "agent-$n has no server on $port — run driver.sh up first" >&2; exit 1; }
    body="$(python3 -c 'import json,sys; print(json.dumps({"command": sys.argv[1]}))' "$1")"
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
    # Stop exactly what `up` recorded. A fixed port window would strand
    # servers whenever the run had more agents than the window covers, and the
    # manifest is deleted here, so nothing would ever find them again.
    ports=""
    if [ -f "$MANIFEST" ]; then
      ports="$(python3 -c 'import json,sys
for e in json.load(open(sys.argv[1])): print(e["port"])' "$MANIFEST")"
    else
      # No manifest (a crashed run, or `down` before `up`): fall back to the
      # default window so stray servers are still reachable.
      ports="$(seq "$PORT_BASE" $((PORT_BASE + 31)))"
    fi
    stopped=0
    for port in $ports; do
      pid="$(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)"
      [ -n "$pid" ] && { kill $pid 2>/dev/null || true; stopped=$((stopped + 1)); }
    done
    rm -f "$MANIFEST"
    echo "stopped $stopped server(s)"
    ;;

  *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
