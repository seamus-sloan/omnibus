#!/usr/bin/env bash
# Run an omnibus-ios test suite (unit | ui | all) against the newest
# available iPhone simulator. Shared by CI (.github/workflows/ios-tests.yml)
# and the local `just ios-test*` recipes so both run the identical invocation.
#
# Usage: ios-test.sh <unit|ui|all> [extra xcodebuild args...]
# Env:   OMNIBUS_IOS_RESULTS_DIR  where <suite>.xcresult lands
#                                 (default .claude/runtime/ios-tests)
#        OMNIBUS_IOS_SIM_UDID     pin a specific simulator instead of
#                                 auto-picking the newest iPhone runtime
set -euo pipefail

suite="${1:?usage: ios-test.sh <unit|ui|all> [xcodebuild args...]}"
shift
case "$suite" in
  unit) only=("-only-testing:omnibusTests") ;;
  ui) only=("-only-testing:omnibusUITests") ;;
  all) only=() ;;
  *)
    echo "ios-test.sh: unknown suite '$suite' (want unit|ui|all)" >&2
    exit 2
    ;;
esac

# Fail with an actionable message rather than a confusing mid-pipeline
# "command not found" from the simulator picker below.
if ! command -v jq >/dev/null 2>&1; then
  echo "ios-test.sh: jq is required (brew install jq, or any nix dev shell)" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
results_dir="${OMNIBUS_IOS_RESULTS_DIR:-$repo_root/.claude/runtime/ios-tests}"
mkdir -p "$results_dir"
# xcodebuild refuses to write over an existing result bundle.
rm -rf "$results_dir/$suite.xcresult"

if [ -n "${OMNIBUS_IOS_SIM_UDID:-}" ]; then
  udid="$OMNIBUS_IOS_SIM_UDID"
else
  # Newest installed iOS runtime that has an iPhone device. xcodebuild's
  # destination matcher silently omits simulators whose runtime is older than
  # the project's IPHONEOS_DEPLOYMENT_TARGET (they don't even show up as
  # ineligible), so pick a concrete UDID ourselves and fail with a message
  # that points at the real cause.
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
  echo "ios-test.sh: no available iPhone simulator; if 'xcrun simctl list devices' shows iPhones, their iOS runtime is older than the project's IPHONEOS_DEPLOYMENT_TARGET" >&2
  exit 1
fi

exec xcodebuild test \
  -project "$repo_root/omnibus-ios/omnibus.xcodeproj" \
  -scheme omnibus \
  -destination "platform=iOS Simulator,id=$udid" \
  -enableCodeCoverage YES \
  -resultBundlePath "$results_dir/$suite.xcresult" \
  ${only[@]+"${only[@]}"} \
  "$@"
