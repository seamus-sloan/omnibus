#!/usr/bin/env bash
# Turn the unsigned .app that `dx bundle --platform ios --release` emits into
# a signed, App-Store-valid .ipa ready for `xcrun altool` upload.
#
# `dx` has no built-in signing (Dioxus #3817), so this script does the manual
# post-build chain: patch Info.plist -> embed the provisioning profile ->
# codesign with the distribution identity -> zip into a Payload/ .ipa.
#
# Inputs (all via env, so the workflow stays thin):
#   SIGNING_IDENTITY   codesign identity, e.g. "Apple Distribution: Name (TEAMID)"
#   PROVISIONING_PROFILE  path to the decoded .mobileprovision
#   BUILD_NUMBER       CFBundleVersion to stamp (monotonic; CI run number)
#   APP_DIR            optional; the .app to sign. Defaults to the sole
#                      target/dx/omnibus-mobile/release/ios/*.app
#   IPA_OUT            optional output path (default ./omnibus-mobile.ipa)
#
# Output: prints the final .ipa path on stdout as APP_IPA=<path>.
set -euo pipefail

die() { echo "ios-package-ipa: $*" >&2; exit 1; }

: "${SIGNING_IDENTITY:?SIGNING_IDENTITY is required}"
: "${PROVISIONING_PROFILE:?PROVISIONING_PROFILE is required}"
: "${BUILD_NUMBER:?BUILD_NUMBER is required}"

# BUILD_NUMBER is interpolated into PlistBuddy commands; CFBundleVersion is
# numeric (optionally dot-separated). Reject anything else so a stray value
# can't malform the plist or inject command arguments.
case "$BUILD_NUMBER" in
  '' | *[!0-9.]*) die "BUILD_NUMBER must be numeric/dot-separated, got: '$BUILD_NUMBER'" ;;
esac

plistbuddy=/usr/libexec/PlistBuddy
[ -x "$plistbuddy" ] || die "PlistBuddy not found at $plistbuddy (need macOS)"
[ -f "$PROVISIONING_PROFILE" ] || die "provisioning profile not found: $PROVISIONING_PROFILE"

ios_out="target/dx/omnibus-mobile/release/ios"
app_dir="${APP_DIR:-}"
if [ -z "$app_dir" ]; then
  # dx title-cases the bundle name (omnibus-mobile -> OmnibusMobile.app), so
  # glob rather than hardcode. Exactly one .app is expected.
  shopt -s nullglob
  candidates=("$ios_out"/*.app)
  shopt -u nullglob
  [ "${#candidates[@]}" -eq 1 ] || die "expected exactly one .app in $ios_out, found ${#candidates[@]}"
  app_dir="${candidates[0]}"
fi
[ -d "$app_dir" ] || die "app bundle not found: $app_dir"
echo "ios-package-ipa: signing $app_dir"

info_plist="$app_dir/Info.plist"
[ -f "$info_plist" ] || die "Info.plist missing in $app_dir"

# dx omits DTPlatformName; App Store validation rejects the build without it
# (Dioxus #3817). Set the device platform, a floor OS, and the build number.
# `Set` fails on an absent key, so Add-then-Set each.
plist_set() {
  local key="$1" type="$2" value="$3"
  # Quote the value inside the PlistBuddy command so a value with spaces or
  # shell-significant characters can't split the argument or inject commands.
  "$plistbuddy" -c "Add :$key $type \"$value\"" "$info_plist" 2>/dev/null \
    || "$plistbuddy" -c "Set :$key \"$value\"" "$info_plist"
}
plist_set DTPlatformName string iphoneos
# App Store validation rejects the bundle without CFBundlePackageType=APPL
# ("Invalid Bundle OS Type code"); dx omits it like DTPlatformName.
plist_set CFBundlePackageType string APPL
plist_set CFBundleVersion string "$BUILD_NUMBER"
# Only add a floor if the build didn't already declare one.
"$plistbuddy" -c "Print :MinimumOSVersion" "$info_plist" >/dev/null 2>&1 \
  || "$plistbuddy" -c "Add :MinimumOSVersion string 13.0" "$info_plist"
plutil -lint "$info_plist" >/dev/null || die "Info.plist failed plutil lint after patching"

# Embed the provisioning profile the app is signed against.
cp "$PROVISIONING_PROFILE" "$app_dir/embedded.mobileprovision"

# Extract the profile's entitlements so the signature matches what the profile
# authorizes (app-id, team-id, aps-environment). `security cms -D` decodes the
# CMS-wrapped plist; PlistBuddy pulls the Entitlements dict out of it.
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
profile_plist="$workdir/profile.plist"
entitlements="$workdir/entitlements.plist"
security cms -D -i "$PROVISIONING_PROFILE" > "$profile_plist" 2>/dev/null \
  || die "failed to decode provisioning profile"
"$plistbuddy" -x -c "Print :Entitlements" "$profile_plist" > "$entitlements" \
  || die "provisioning profile has no Entitlements dict"

codesign --force --timestamp=none \
  --sign "$SIGNING_IDENTITY" \
  --entitlements "$entitlements" \
  "$app_dir"
codesign --verify --deep --strict "$app_dir" || die "codesign verification failed"

# Package: Payload/<App>.app -> zip -> .ipa (the App Store .ipa layout).
ipa_out="${IPA_OUT:-$PWD/omnibus-mobile.ipa}"
payload_root="$workdir/pkg"
mkdir -p "$payload_root/Payload"
cp -R "$app_dir" "$payload_root/Payload/"
rm -f "$ipa_out"
( cd "$payload_root" && zip -qry "$ipa_out" Payload )
[ -f "$ipa_out" ] || die "failed to produce ipa at $ipa_out"

echo "ios-package-ipa: wrote $ipa_out"
echo "APP_IPA=$ipa_out"
