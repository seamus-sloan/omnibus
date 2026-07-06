#!/usr/bin/env bash
# Build, brand, sign, and install the Omnibus iOS app on a connected iPhone.
#
# Why this is a script and not just `dx`: dx builds + auto-provisions +
# codesigns a device `.app`, but it installs no launcher icon and can't express
# the App Transport Security exception our plain-http LAN server needs. So we
# let dx build+sign, inject the icon + ATS, re-sign (editing the bundle voids
# dx's signature), then install with devicectl.
#
# One-time prerequisites (see docs in the PR / just install-ios comment):
#   1. Apple Developer Program membership.
#   2. Your Apple ID added in Xcode ▸ Settings ▸ Accounts, which creates an
#      "Apple Development" certificate. Verify: security find-identity -v -p codesigning
#   3. iPhone connected over USB and "Trust This Computer" accepted.
#
# Usage: scripts/install-ios.sh "<device name>"   (defaults to the first usable device)
#
# Verified end-to-end on an iPhone 17: device build → dx sign → icon + ATS
# injection → re-sign → devicectl install.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
: "${DEVELOPER_DIR:=/Applications/Xcode.app/Contents/Developer}"
export DEVELOPER_DIR
# CARGO_TARGET_DIR is normally exported by the nix dev shell; default it to the
# per-worktree path the flake uses so `set -u` doesn't trip when run bare.
: "${CARGO_TARGET_DIR:="$HOME/.cache/cargo-target/$(basename "$REPO_ROOT")"}"
export CARGO_TARGET_DIR

# Resolve the target device (name + identifier) from devicectl. Accept an
# installable state (connected, or available/paired); never a disconnected,
# unavailable, or unpaired device. `|| true` keeps `set -e` from killing us on a
# no-match so the explicit check below can print a useful error.
DEVICE_NAME="${1:-}"
DEVICE_LIST="$(xcrun devicectl list devices 2>/dev/null || true)"
DEV_LINE="$(printf '%s\n' "$DEVICE_LIST" \
  | grep -viE 'disconnected|unavailable|unpaired' \
  | { [ -n "$DEVICE_NAME" ] && grep -F "$DEVICE_NAME" || grep -iE 'connected|available'; } \
  | head -1 || true)"
[ -n "$DEV_LINE" ] || { echo "error: no usable iPhone found via devicectl (plug in, unlock, and trust the Mac)." >&2; exit 1; }
DEVICE_NAME="${DEVICE_NAME:-$(echo "$DEV_LINE" | sed -E 's/   +.*//')}"
DEVICE_ID="$(echo "$DEV_LINE" | grep -oiE '[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}' | head -1)"

# Preflight: a signing identity must exist (this is the enrollment gate).
IDENTITY="$(security find-identity -v -p codesigning | grep -Eo 'Apple (Development|Distribution): [^"]+' | head -1 || true)"
[ -n "$IDENTITY" ] || {
  echo "error: no Apple codesigning identity in the keychain." >&2
  echo "       Enroll in the Apple Developer Program, then add your Apple ID in" >&2
  echo "       Xcode ▸ Settings ▸ Accounts to create an 'Apple Development' cert." >&2
  exit 1
}
echo "device:   $DEVICE_NAME ($DEVICE_ID)"
echo "identity: $IDENTITY"

# 1. Build for the device and let dx auto-provision + codesign. `--package
#    omnibus-mobile` is required — without it dx builds the default `omnibus`
#    (server/web) crate for iOS, which fails to compile.
dx build --platform ios --package omnibus-mobile --device "$DEVICE_NAME" --codesign

# 2. Locate the freshly built device .app (device + simulator share the name in
#    different arch dirs, so take the newest under the dx output tree).
APP="$(find "$CARGO_TARGET_DIR/dx/omnibus-mobile" -type d -name '*.app' -path '*ios*' -print0 2>/dev/null \
        | xargs -0 ls -dt 2>/dev/null | head -1)"
[ -n "$APP" ] && [ -d "$APP" ] || { echo "error: built .app not found under dx output" >&2; exit 1; }
echo "app:      $APP"

# 3. Preserve the entitlements dx signed with, so the re-sign keeps provisioning intact.
ENT="$(mktemp -t omnibus-ents)"
codesign -d --entitlements "$ENT" --xml "$APP" 2>/dev/null || ENT=""

# 4. Inject the launcher icon (device SDK) and the ATS cleartext-http exception.
scripts/apply-ios-icon.sh "$APP" iphoneos
PLIST="$APP/Info.plist"
# Add an ATS cleartext-http exception. Only touch NSAllowsArbitraryLoads, and
# create the NSAppTransportSecurity dict only if absent, so we don't clobber any
# sibling ATS keys. Idempotent (Set if present, else Add).
/usr/libexec/PlistBuddy -c "Print :NSAppTransportSecurity" "$PLIST" >/dev/null 2>&1 \
  || /usr/libexec/PlistBuddy -c "Add :NSAppTransportSecurity dict" "$PLIST"
/usr/libexec/PlistBuddy -c "Set :NSAppTransportSecurity:NSAllowsArbitraryLoads true" "$PLIST" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c "Add :NSAppTransportSecurity:NSAllowsArbitraryLoads bool true" "$PLIST"

# 5. Re-sign the modified bundle (editing it invalidated dx's signature).
codesign --force --sign "$IDENTITY" ${ENT:+--entitlements "$ENT"} --timestamp=none "$APP"

# 6. Install to the phone.
xcrun devicectl device install app --device "${DEVICE_ID:-$DEVICE_NAME}" "$APP"
echo "installed $APP on $DEVICE_NAME — launch it from the home screen."
