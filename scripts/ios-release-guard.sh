#!/usr/bin/env bash
# Prove the app's DEBUG-only hooks are absent from a Release build.
#
# The simulated-offline switch (omnibus-ios/omnibus/Offline/DebugOffline.swift)
# takes the app off the network on an `omnibus://debug/offline` URL, which the
# agentic-exploration iOS lane drives with `simctl openurl`. That is fine in a
# debug build and unacceptable in a shipped one, so "it's inside #if DEBUG" is
# not a claim to take on trust — this checks the built binary.
#
# What it can and cannot see: it looks for the string literals those hooks are
# addressed by, so a `#if DEBUG` block that leaked into Release is caught as
# soon as it is reachable — verified by wiring the marker into a live view and
# watching this fail. Dead code is stripped before it gets here (an *unused*
# leaked constant does not trip it, correctly), and a leak that carries no
# literal of its own would not be seen at all. It is a tripwire on the hooks
# named below, not a general proof that Release contains no debug code.
#
# Usage: ios-release-guard.sh [extra xcodebuild args...]
# Env:   OMNIBUS_IOS_RELEASE_DERIVED_DIR  derived data location
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

# Same reason as scripts/ios-test.sh: the nix dev shells export LD/CC/CXX for
# cargo, xcodebuild adopts $LD as its link driver, and raw ld rejects the
# clang-style args it then receives.
unset LD CC CXX

# Every marker a Release binary must not contain. Each is a string literal that
# exists only inside a `#if DEBUG` block; if one shows up in the Release binary,
# the code around it shipped.
markers=(
  "omnibus.debug.forcedOffline"
  "--uitest-offline"
  "--uitest-online"
  "--uitest-reset"
  "--uitest-shell"
)

# A guard that cannot fail is not a guard. If a marker has been renamed in the
# source, absence from the Release binary proves nothing — so require each one
# to still exist in the sources before trusting its absence in the build.
missing_in_source=0
for marker in "${markers[@]}"; do
  if ! grep -rqF -- "$marker" "$repo_root/omnibus-ios/omnibus"; then
    echo "ios-release-guard: marker '$marker' is in no source file — rename it here too" >&2
    missing_in_source=1
  fi
done
[ "$missing_in_source" -eq 0 ] || exit 1

derived="${OMNIBUS_IOS_RELEASE_DERIVED_DIR:-$HOME/.cache/omnibus-ios-derived/release-guard}"
mkdir -p "$(dirname "$derived")"

echo "building Release (simulator, unsigned)…" >&2
xcodebuild build \
  -project "$repo_root/omnibus-ios/omnibus.xcodeproj" -scheme omnibus \
  -configuration Release \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath "$derived" \
  CODE_SIGNING_ALLOWED=NO \
  "$@" >"$derived.log" 2>&1 || {
    echo "ios-release-guard: Release build failed — see $derived.log" >&2
    tail -30 "$derived.log" >&2
    exit 1
  }

app="$derived/Build/Products/Release-iphonesimulator/omnibus.app"
[ -d "$app" ] || { echo "ios-release-guard: no app at $app" >&2; exit 1; }

# Every Mach-O in the bundle, not just the main executable: a Debug build puts
# the app's own code in a sidecar `omnibus.debug.dylib`, so checking one file
# by name is exactly the mistake that would make this pass vacuously.
# A read loop rather than `mapfile`, which macOS's own bash 3.2 does not have,
# and one `file` call per path rather than a batch: a universal binary makes
# `file` print a line per architecture, and splitting those on ':' yields
# "…/omnibus (for architecture arm64)" — paths that do not exist, whose
# `strings` failure would then read as the marker being absent.
binaries=()
while IFS= read -r candidate; do
  [ -n "$candidate" ] || continue
  case "$(file -b "$candidate" | head -1)" in
    *Mach-O*) binaries+=("$candidate") ;;
  esac
done < <(find "$app" -type f)
[ "${#binaries[@]}" -gt 0 ] || { echo "ios-release-guard: no Mach-O found in $app" >&2; exit 1; }

# Positive control. Absence proves nothing if `strings` is reading the wrong
# file or reading nothing at all, so require a literal that must be in a
# working Release build before trusting any of the absences below.
control="Connect to Omnibus"
control_found=0
for binary in "${binaries[@]}"; do
  # `grep -F` without `-q`: `-q` exits on the first match, which SIGPIPEs
  # `strings`, and `pipefail` then reports the pipeline as failed — turning
  # every match into a miss. Reading to EOF is the whole fix.
  if strings -a "$binary" | grep -F -- "$control" >/dev/null; then control_found=1; fi
done
[ "$control_found" -eq 1 ] || {
  echo "ios-release-guard: control string '$control' found in none of the"     \
       "${#binaries[@]} binary/binaries — the scan is not reading the app," >&2
  echo "so the absences below would be meaningless. Fix the scan." >&2
  exit 1
}

status=0
for marker in "${markers[@]}"; do
  hits=""
  for binary in "${binaries[@]}"; do
    if strings -a "$binary" | grep -F -- "$marker" >/dev/null; then
      hits="$hits $(basename "$binary")"
    fi
  done
  if [ -n "$hits" ]; then
    echo "  FAIL  $marker  present in:$hits" >&2
    status=1
  else
    echo "  ok    $marker  absent from ${#binaries[@]} binary/binaries"
  fi
done

if [ "$status" -ne 0 ]; then
  echo "ios-release-guard: a DEBUG-only hook reached the Release build" >&2
  exit 1
fi
echo "ios-release-guard: clean"
