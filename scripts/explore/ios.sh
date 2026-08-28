#!/usr/bin/env bash
# iOS lane driver for the agentic-exploration swarm: one simulator running the
# native SwiftUI app, plus the DEBUG-only offline switch and the two readbacks
# that make an offline scenario checkable rather than merely performed.
#
# Usage:
#   ios.sh up [--offline]      boot a simulator, build, install, launch
#   ios.sh offline on|off      flip the simulated-offline switch, verified
#   ios.sh offline-url on|off  the same flip without a relaunch — needs a tap
#   ios.sh state               {online, running, forced_offline} as JSON
#   ios.sh outbox              the queued mutations, as JSON
#   ios.sh relaunch            kill and relaunch, container and switch intact
#   ios.sh screenshot [path]   PNG of the current screen
#   ios.sh down                terminate the app (the simulator stays booted)
#
# Two ways to flip the switch, and the difference matters:
#
#   `offline` relaunches the app with `--uitest-offline` / `--uitest-online`.
#   Wholly non-interactive, which is the requirement — so it is the default.
#   The cost is the relaunch: sequence a scenario so the switch moves *between*
#   screens rather than in the middle of one.
#
#   `offline-url` sends `omnibus://debug/offline?on=…` to the running app, with
#   no relaunch. On iOS 26 the system puts up an "Open in Omnibus?" confirmation
#   for any externally-opened custom scheme, and `simctl` cannot dismiss it —
#   something has to tap Open. Use it only when you can drive the screen.
#
# One simulator, one agent. Two agents on one simulator would share a keychain,
# a container and a session — the exact collapse that killed run
# r-20260828-01 on the web side, with no per-agent isolation available to fix
# it. The harness enforces the limit; this script does not multiplex.
#
# Everything here is system Xcode (xcodebuild, simctl) — no nix shell, unlike
# the web driver, which needs the flake's Chromium.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
STATE="$ROOT/.claude/runtime/explore/ios"
MANIFEST="$STATE/manifest.json"

# The nix dev shells export LD/CC/CXX for cargo; xcodebuild adopts $LD as its
# link driver and raw ld rejects the clang-style args it then receives.
unset LD CC CXX

ios::manifest_field() {
  [ -f "$MANIFEST" ] || { echo "no simulator yet — run ios.sh up" >&2; return 1; }
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' \
    "$MANIFEST" "$1"
}

ios::udid() { ios::manifest_field udid; }
ios::bundle() { ios::manifest_field bundle_id; }

# Where the app's own files live. iOS reassigns this UUID on reinstall, so it
# is resolved every time rather than recorded in the manifest.
ios::container() {
  xcrun simctl get_app_container "$(ios::udid)" "$(ios::bundle)" data
}

ios::is_running() {
  # `simctl spawn launchctl list` names every running process in the
  # simulator's own domain. `simctl launch` would *start* the app, which is
  # the opposite of asking whether it is up.
  xcrun simctl spawn "$(ios::udid)" launchctl list 2>/dev/null \
    | grep -F "UIKitApplication:$(ios::bundle)" >/dev/null
}

# The persisted switch, read out of the app's own preferences plist.
#
# Not `simctl spawn … defaults read`: that answers from a preferences cache
# that lags the app's own writes by an unbounded amount, and it reported this
# app online for minutes after it had gone offline — and reported a cleared
# server still set. `DebugOffline.write` flushes the plist precisely so this
# read is current. Answers true / false / unknown.
ios::forced_offline() {
  local plist
  plist="$(ios::container)/Library/Preferences/$(ios::bundle).plist"
  [ -f "$plist" ] || { echo false; return; }
  # A key that was never written is the app's own default: a container nothing
  # has switched is online.
  python3 -c 'import plistlib,sys
try:
    with open(sys.argv[1], "rb") as h:
        v = plistlib.load(h).get("omnibus.debug.forcedOffline")
except Exception:
    print("unknown")
else:
    print("false" if v is None else ("true" if v else "false"))' "$plist"
}

cmd="${1:?usage: ios.sh up | offline on|off | offline-url on|off | state | outbox | relaunch | screenshot | down}"
shift || true

case "$cmd" in
  up)
    start_offline=0
    [ "${1-}" = "--offline" ] && start_offline=1
    command -v jq >/dev/null 2>&1 \
      || { echo "jq is required (brew install jq, or any nix dev shell)" >&2; exit 1; }

    if [ -n "${OMNIBUS_IOS_SIM_UDID:-}" ]; then
      udid="$OMNIBUS_IOS_SIM_UDID"
    else
      # Newest installed iOS runtime carrying an iPhone. Same picker as
      # scripts/ios-test.sh and scripts/ios-sim.sh — xcodebuild's destination
      # matcher silently omits runtimes older than the deployment target, so
      # resolve a concrete udid ourselves rather than letting it choose.
      udid="$(xcrun simctl list --json devices available | jq -r '
        .devices | to_entries
        | map(select(.key | test("SimRuntime\\.iOS")))
        | map({ver: (.key | sub(".*iOS-"; "") | gsub("-"; ".")),
               devs: [.value[] | select(.name | startswith("iPhone"))]})
        | map(select(.devs | length > 0))
        | sort_by(.ver | split(".") | map(tonumber))
        | if length == 0 then empty else last.devs[0].udid end')"
    fi
    [ -n "$udid" ] || {
      echo "no available iPhone simulator — check 'xcrun simctl list devices'" >&2
      exit 1
    }

    mkdir -p "$STATE"
    xcrun simctl bootstatus "$udid" -b
    derived="${OMNIBUS_IOS_DERIVED_DIR:-$HOME/.cache/omnibus-ios-derived/$(basename "$ROOT")}"
    # Debug pinned explicitly: the scheme's Run action is Release, and Release
    # is precisely the build with no offline switch in it.
    xcodebuild build \
      -project "$ROOT/omnibus-ios/omnibus.xcodeproj" -scheme omnibus \
      -configuration Debug -destination "platform=iOS Simulator,id=$udid" \
      -derivedDataPath "$derived" >"$STATE/build.log" 2>&1 || {
        echo "build failed — see $STATE/build.log" >&2; tail -30 "$STATE/build.log" >&2; exit 1
      }
    app="$derived/Build/Products/Debug-iphonesimulator/omnibus.app"
    [ -d "$app" ] || { echo "built app not found at $app" >&2; exit 1; }
    # Read off the built product, so a project-level rename can't silently
    # break the launch and container lookups.
    bundle_id="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Info.plist")"

    xcrun simctl install "$udid" "$app"
    python3 -c 'import json,sys; print(json.dumps(
        {"udid": sys.argv[1], "bundle_id": sys.argv[2], "app": sys.argv[3]}))' \
      "$udid" "$bundle_id" "$app" >"$MANIFEST"

    arg="--uitest-online"
    [ "$start_offline" -eq 1 ] && arg="--uitest-offline"
    xcrun simctl launch --terminate-running-process "$udid" "$bundle_id" "$arg" >/dev/null
    cat "$MANIFEST"
    ;;

  offline|offline-url)
    want="${1:?usage: ios.sh $cmd on|off}"
    case "$want" in
      on) flag=1; expect=true ;;
      off) flag=0; expect=false ;;
      *) echo "usage: ios.sh $cmd on|off" >&2; exit 2 ;;
    esac
    if [ "$cmd" = "offline" ]; then
      arg="--uitest-offline"
      [ "$flag" = 0 ] && arg="--uitest-online"
      xcrun simctl launch --terminate-running-process \
        "$(ios::udid)" "$(ios::bundle)" "$arg" >/dev/null
    else
      ios::is_running || { echo "the app is not running — run ios.sh up" >&2; exit 1; }
      # `openurl` reports that iOS accepted the URL, never that the app
      # understood it, so its exit code says nothing about the switch.
      xcrun simctl openurl "$(ios::udid)" "omnibus://debug/offline?on=$flag" >/dev/null
    fi

    # Poll the readback either way. A scenario that believes it went offline
    # and did not is the one failure mode that produces a clean-looking pass
    # over a test that never ran.
    for _ in $(seq 1 40); do
      if [ "$(ios::forced_offline)" = "$expect" ]; then
        echo "{\"online\": $([ "$expect" = true ] && echo false || echo true)}"
        exit 0
      fi
      sleep 0.5
    done
    echo "switch did not take: wanted forcedOffline=$expect, app reports $(ios::forced_offline)" >&2
    [ "$cmd" = "offline-url" ] \
      && echo "(is the system's \"Open in Omnibus?\" prompt waiting for a tap?)" >&2
    exit 1
    ;;

  state)
    forced="$(ios::forced_offline)"
    running=false
    ios::is_running && running=true
    # An unreadable plist must still yield valid JSON — downstream parsers get
    # null, which is "can't tell", never a bare identifier they choke on.
    case "$forced" in
      true)  online=false;   forced_json=true ;;
      false) online=true;    forced_json=false ;;
      *)     online=null;    forced_json=null ;;
    esac
    echo "{\"online\": $online, \"running\": $running, \"forced_offline\": $forced_json}"
    ;;

  outbox)
    db="$(ios::container)/Library/Application Support/Omnibus/offline.sqlite"
    [ -f "$db" ] || { echo "[]"; exit 0; }
    # Read-only, so inspecting the queue can never be what changes it. The DB
    # is in WAL mode and the app holds it open; a reader sees the last
    # committed state, which is exactly the question being asked.
    rows="$(sqlite3 -readonly -json "$db" \
      'SELECT id, kind, method, path, attempts, last_error,
              CAST(body AS TEXT) AS body
       FROM ops ORDER BY id')"
    # `sqlite3 -json` prints *nothing* for an empty result, and a caller piping
    # this into a JSON parser needs an empty array, not an empty string.
    echo "${rows:-[]}"
    ;;

  relaunch)
    # No arguments, deliberately: this is the durability check — kill the app
    # and see that the queue and the switch both survived. Passing an argument
    # here would be setting the state you were about to verify.
    xcrun simctl launch --terminate-running-process \
      "$(ios::udid)" "$(ios::bundle)" >/dev/null
    echo "relaunched"
    ;;

  screenshot)
    out="${1:-$STATE/screen-$(date -u +%Y%m%dT%H%M%SZ).png}"
    mkdir -p "$(dirname "$out")"
    xcrun simctl io "$(ios::udid)" screenshot "$out" >/dev/null 2>&1
    echo "$out"
    ;;

  down)
    xcrun simctl terminate "$(ios::udid)" "$(ios::bundle)" >/dev/null 2>&1 || true
    echo "terminated"
    ;;

  *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
