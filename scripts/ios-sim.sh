#!/usr/bin/env bash
# Boot the newest available iPhone simulator, build the native SwiftUI app
# (omnibus-ios/, scheme `omnibus`), install it, and launch it. Backs the
# `just ios-sim` recipe and the `ios` multiplexer pane. xcodebuild + simctl
# are system Xcode — no nix shell involved.
#
# Env: OMNIBUS_IOS_SIM_UDID     pin a specific simulator instead of
#                               auto-picking the newest iPhone runtime
#      OMNIBUS_IOS_DERIVED_DIR  override the derived-data path (default
#                               ~/.cache/omnibus-ios-derived/<worktree> —
#                               outside the repo, same philosophy as
#                               CARGO_TARGET_DIR)
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
derived="${OMNIBUS_IOS_DERIVED_DIR:-$HOME/.cache/omnibus-ios-derived/$(basename "$repo_root")}"

# The nix dev shell (direnv-loaded into most local shells) exports LD/CC/CXX;
# xcodebuild adopts $LD as the linker driver and raw ld can't parse the
# clang-style args it then receives, so strip them before any xcodebuild call.
unset LD CC CXX

if [ -n "${OMNIBUS_IOS_SIM_UDID:-}" ]; then
  udid="$OMNIBUS_IOS_SIM_UDID"
else
  # Newest installed iOS runtime that has an iPhone device. Duplicated from
  # scripts/ios-test.sh (which CI owns) rather than shared — keep in sync.
  # xcodebuild's destination matcher silently omits simulators whose runtime
  # is older than the project's IPHONEOS_DEPLOYMENT_TARGET, so pick a
  # concrete UDID ourselves and fail with a message naming the real cause.
  udid="$(xcrun simctl list --json devices available | jq -r '
    .devices | to_entries
    | map(select(.key | test("SimRuntime\\.iOS")))
    | map({ver: (.key | sub(".*iOS-"; "") | gsub("-"; ".")),
           devs: [.value[] | select(.name | startswith("iPhone"))]})
    | map(select(.devs | length > 0))
    | sort_by(.ver | split(".") | map(tonumber))
    | if length == 0 then empty else last.devs[0].udid end')"
fi
if [ -z "$udid" ]; then
  echo "ios-sim.sh: no available iPhone simulator; if 'xcrun simctl list devices' shows iPhones, their iOS runtime is older than the project's IPHONEOS_DEPLOYMENT_TARGET" >&2
  exit 1
fi

# -b boots the device if it's shut down and blocks until it finishes booting;
# on an already-booted device it returns immediately.
xcrun simctl bootstatus "$udid" -b
open -a Simulator

# -configuration Debug pinned explicitly: the shared scheme's Run action is
# Release, and the install step below needs a deterministic product path.
xcodebuild build \
  -project "$repo_root/omnibus-ios/omnibus.xcodeproj" \
  -scheme omnibus \
  -configuration Debug \
  -destination "platform=iOS Simulator,id=$udid" \
  -derivedDataPath "$derived"

app="$derived/Build/Products/Debug-iphonesimulator/omnibus.app"
if [ ! -d "$app" ]; then
  echo "ios-sim.sh: built app not found at $app" >&2
  exit 1
fi

# Read the bundle id off the built product rather than hardcoding it, so a
# project-level rename can't silently break the launch step.
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Info.plist")"

xcrun simctl install "$udid" "$app"
xcrun simctl launch "$udid" "$bundle_id"

# The app has no baked-in server URL — hint at this workspace's dev server
# (written by `just dev-up`) for the Connect screen.
if [ -f "$repo_root/.claude/runtime/env.sh" ]; then
  # shellcheck disable=SC1091
  source "$repo_root/.claude/runtime/env.sh"
  echo "Dev server for this workspace: http://localhost:${OMNIBUS_PORT:-3000} — enter it on the Connect screen."
fi
