#!/usr/bin/env bash
# Browser driver for the agentic-exploration swarm: one playwright-repl server
# per agent, each with its own session, port, and browser process.
#
# Usage:
#   driver.sh up <agent-count>     start N servers, print the port manifest
#   driver.sh run <n> <command>    send one command to agent n, print the result
#   driver.sh guard <n> <actor> <uuids>
#                                  enforce ownership for agent n: destructive
#                                  calls to books outside <uuids> are refused
#   driver.sh refusals <n>         what agent n's guard refused
#   driver.sh restart <n>          replace agent n's server after its browser
#                                  died, and put it back in the manifest
#   driver.sh status               which agents are up, plus any driver
#                                  listening in the window unregistered
#   driver.sh down                 stop every server, manifest or not
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
#
# Why `down` and `status` sweep a port window as well as the manifest:
#   A server an agent restarts by hand — which it must, after its browser dies
#   mid-run — was never in the manifest, and run r-20260829-01 left one of
#   those listening after teardown (#2363). The window is the only place a
#   driver can be, so it is swept too; anything on those ports that is not a
#   playwright-repl process is reported and left alone.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
DRIVER="$HERE/driver"
PORT_BASE="${OMNIBUS_EXPLORE_PORT_BASE:-9223}"
# Ports a driver may hold: PORT_BASE .. PORT_BASE + WINDOW - 1.
WINDOW="${OMNIBUS_EXPLORE_PORT_WINDOW:-32}"
STATE="$ROOT/.claude/runtime/explore/driver"
MANIFEST="$STATE/ports.json"
RUN_TIMEOUT=180

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
driver::agent_of_port() { echo $(($1 - PORT_BASE + 1)); }
driver::window_ports() { seq "$PORT_BASE" $((PORT_BASE + WINDOW - 1)); }

# An agent number must land inside the window, or `run`/`guard`/`restart`
# would drive a port that `status` and `down` never sweep.
driver::check_n() {
  [[ "$1" =~ ^[0-9]+$ ]] && [ "$1" -ge 1 ] \
    || { echo "agent number must be a positive integer (got: $1)" >&2; exit 2; }
  [ "$1" -le "$WINDOW" ] \
    || { echo "agent number $1 is outside the $WINDOW-port window — raise OMNIBUS_EXPLORE_PORT_WINDOW" >&2; exit 2; }
}

driver::alive() {
  # 000 means nothing accepted the connection; any HTTP code means a server is
  # there. `/` is deliberately not a route, so 404 is the healthy answer.
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://127.0.0.1:$1/" 2>/dev/null)" || true
  [ -n "$code" ] && [ "$code" != "000" ]
}

# Pids listening on a port, one per line; empty when nothing is.
driver::listener_pids() { lsof -tiTCP:"$1" -sTCP:LISTEN 2>/dev/null || true; }

# Whether a pid is one of our servers. Everything else on a driver port —
# somebody's dev server, a stray node — is not ours to kill.
driver::is_driver() { ps -o command= -p "$1" 2>/dev/null | grep -q playwright-repl; }

# Stop one of our servers: TERM, a few seconds' grace for the browser to close,
# then KILL — a server that lingers is exactly what `down` exists to prevent.
driver::stop_pid() {
  kill "$1" 2>/dev/null || true
  for _ in $(seq 1 10); do kill -0 "$1" 2>/dev/null || return 0; sleep 0.5; done
  kill -KILL "$1" 2>/dev/null || true
}

# Ports the manifest records, one per line; nothing when there is no manifest.
driver::manifest_ports() {
  [ -f "$MANIFEST" ] || return 0
  python3 -c 'import json,sys
for e in json.load(open(sys.argv[1])): print(e["port"])' "$MANIFEST"
}

# Start agent n's server on its port and wait for it to answer.
driver::start() {
  local n="$1" port="$2"
  (cd "$DRIVER" && \
    nohup ./node_modules/.bin/playwright-repl --silent --http \
      --http-port "$port" -s "agent-$n" \
      >"$STATE/agent-$n.log" 2>&1 &)
  for _ in $(seq 1 40); do driver::alive "$port" && break; sleep 1; done
  driver::alive "$port" \
    || { echo "agent-$n failed to start on $port — see $STATE/agent-$n.log" >&2; exit 1; }
}

# Record agent n's port in the manifest, creating or updating its entry.
driver::register() {
  local n="$1" port="$2"
  mkdir -p "$STATE"
  python3 -c 'import json,sys
path, actor, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
try:
    entries = json.load(open(path))
except (OSError, ValueError):
    entries = []
entries = [e for e in entries if e.get("actor") != actor and e.get("port") != port]
entries.append({"actor": actor, "port": port})
entries.sort(key=lambda e: e["port"])
json.dump(entries, open(path, "w"))
open(path, "a").write("\n")' "$MANIFEST" "agent-$n" "$port"
}

# One JSON line in the shape `run` prints, so an agent parsing `text` and
# `isError` sees a driver verdict the same way it sees a command result.
driver::verdict() {
  python3 -c 'import json,sys
print(json.dumps({"text": sys.argv[1], "isError": True, "driver": sys.argv[2]}))' "$1" "$2"
}

cmd="${1:?usage: driver.sh up <n> | run <n> <command> | guard <n> <actor> <uuids> | refusals <n> | restart <n> | status | down}"

case "$cmd" in
  up)
    count="${2:?usage: driver.sh up <agent-count>}"
    [[ "$count" =~ ^[0-9]+$ ]] && [ "$count" -ge 1 ] \
      || { echo "agent-count must be a positive integer" >&2; exit 2; }
    [ "$count" -le "$WINDOW" ] \
      || { echo "agent-count $count exceeds the $WINDOW-port window — raise OMNIBUS_EXPLORE_PORT_WINDOW" >&2; exit 2; }
    driver::ensure_deps
    mkdir -p "$STATE"
    entries=""
    for i in $(seq 1 "$count"); do
      port="$(driver::port "$i")"
      if driver::alive "$port"; then
        echo "  agent-$i: reusing server on $port" >&2
      else
        driver::start "$i" "$port"
        echo "  agent-$i: started on $port" >&2
      fi
      entries="$entries{\"actor\":\"agent-$i\",\"port\":$port},"
    done
    printf '[%s]\n' "${entries%,}" | tee "$MANIFEST"
    ;;

  run)
    n="${2:?usage: driver.sh run <n> \"<command>\"}"
    driver::check_n "$n"
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
    rc=0
    out="$(curl -sS --max-time "$RUN_TIMEOUT" -X POST "http://127.0.0.1:$port/run" \
      -H 'Content-Type: application/json' -d "$body")" || rc=$?
    if [ "$rc" -ne 0 ]; then
      # Tell a dead driver from a hung app (#2361): the server answering `/`
      # after the failure means the browser is fine and the command itself
      # never returned; a server that is gone died under the command.
      if driver::alive "$port"; then
        if [ "$rc" -eq 28 ]; then
          driver::verdict "driver.sh: no answer within ${RUN_TIMEOUT}s, but agent-$n's server on $port is still up — the app or a waiting locator hung, not the driver. Reload and retry; an app hang past thirty seconds is a finding." up
        else
          driver::verdict "driver.sh: could not reach agent-$n's server on $port (curl exit $rc), yet it is up — retry once before treating this as anything." up
        fi
      else
        driver::verdict "driver.sh: agent-$n's browser server on port $port died while running this command. That is the harness, not the app — journal an anomaly of kind 'issue', run 'driver.sh restart $n', re-guard, and repeat the step. Log: $STATE/agent-$n.log" dead
      fi
      exit 1
    fi
    printf '%s\n' "$out"
    ;;

  guard)
    # Enforce the ownership rule instead of trusting the agent to follow it.
    # start.md says an agent may only destroy what it added, but every
    # exploration account is an admin, so the server will not stop agent-2
    # deleting agent-5's book. Wrapping `fetch` in the agent's own browser
    # refuses the request before it is sent, which is the difference between a
    # rule and a convention. (In the page, not via `page.route()` — see the
    # header of driver/guard.js for why that killed large uploads.)
    n="${2:?usage: driver.sh guard <n> <actor> <comma-separated-uuids>}"
    actor="${3:?usage: driver.sh guard <n> <actor> <comma-separated-uuids>}"
    uuids="${4-}"
    driver::check_n "$n"
    port="$(driver::port "$n")"
    driver::alive "$port" || { echo "agent-$n has no server on $port — run driver.sh up first" >&2; exit 1; }

    owned_json="$(python3 -c 'import json,sys
raw = sys.argv[1] if len(sys.argv) > 1 else ""
print(json.dumps([u for u in raw.split(",") if u.strip()]))' "$uuids")"
    js="$(python3 -c 'import sys
src = open(sys.argv[1]).read()
print(src.replace("__OWNED__", sys.argv[2]).replace("__ACTOR__", sys.argv[3]))' \
      "$DRIVER/guard.js" "$owned_json" "$actor")"
    body="$(python3 -c 'import json,sys; print(json.dumps({"command": sys.argv[1]}))' "await $js")"
    curl -sS --max-time 60 -X POST "http://127.0.0.1:$port/run" \
      -H 'Content-Type: application/json' -d "$body" >/dev/null
    echo "  agent-$n guarded: $(printf '%s' "$owned_json" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))') owned book(s)"
    ;;

  refusals)
    # What the guard actually blocked. A non-empty list is a finding about the
    # agent or the flow doc, not about the app.
    n="${2:?usage: driver.sh refusals <n>}"
    driver::check_n "$n"
    port="$(driver::port "$n")"
    curl -sS --max-time 30 -X POST "http://127.0.0.1:$port/run" \
      -H 'Content-Type: application/json' \
      -d '{"command":"globalThis.__omnibusGuardRefusals || []"}'
    echo
    ;;

  restart)
    # A fresh server for one agent, registered so `status` and `down` know
    # about it. The guard lives in the old server's process and does not come
    # back on its own — hence the reminder.
    n="${2:?usage: driver.sh restart <n>}"
    driver::check_n "$n"
    driver::ensure_deps
    mkdir -p "$STATE"
    port="$(driver::port "$n")"
    for pid in $(driver::listener_pids "$port"); do
      driver::is_driver "$pid" \
        || { echo "port $port is held by a process that is not a driver (pid $pid) — not touching it" >&2; exit 1; }
      driver::stop_pid "$pid"
    done
    for _ in $(seq 1 20); do driver::alive "$port" || break; sleep 0.5; done
    driver::alive "$port" && { echo "agent-$n's old server on $port would not stop" >&2; exit 1; }
    driver::start "$n" "$port"
    driver::register "$n" "$port"
    echo "  agent-$n: restarted on $port and registered"
    echo "  the guard did not survive — re-run: driver.sh guard $n agent-$n \"\$(scripts/explore/owned.sh agent-$n)\"" >&2
    ;;

  status)
    listed=""
    if [ -f "$MANIFEST" ]; then
      while read -r actor port; do
        if driver::alive "$port"; then state=up; else state=DOWN; fi
        printf '  %-9s port %s  %s\n' "$actor" "$port" "$state"
        listed="$listed $port"
      done < <(python3 -c 'import json,sys
for e in json.load(open(sys.argv[1])): print(e["actor"], e["port"])' "$MANIFEST")
    fi
    # A driver listening on a window port the manifest does not know about is
    # a hand-restarted server: `down` will still stop it, but name it here so
    # nobody wonders which agent it belongs to.
    strays=0
    for port in $(driver::window_ports); do
      case " $listed " in *" $port "*) continue ;; esac
      driver::alive "$port" || continue
      strays=$((strays + 1))
      printf '  %-9s port %s  up (unregistered — driver.sh restart %s re-registers it)\n' \
        "agent-$(driver::agent_of_port "$port")" "$port" "$(driver::agent_of_port "$port")"
    done
    [ -f "$MANIFEST" ] || [ "$strays" -gt 0 ] || echo "no manifest — nothing started"
    ;;

  down)
    # Stop what `up` recorded and anything else of ours in the window, so a
    # server restarted by hand cannot outlive the run. A port in neither is
    # not somewhere a driver can be.
    ports="$( { driver::manifest_ports; driver::window_ports; } | sort -un)"
    stopped=0
    kept=""
    for port in $ports; do
      for pid in $(driver::listener_pids "$port"); do
        if driver::is_driver "$pid"; then
          driver::stop_pid "$pid"
          stopped=$((stopped + 1))
        else
          kept="$kept $port(pid $pid)"
        fi
      done
    done
    rm -f "$MANIFEST"
    echo "stopped $stopped server(s)"
    [ -z "$kept" ] || echo "left alone, not a driver:$kept" >&2
    ;;

  *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
