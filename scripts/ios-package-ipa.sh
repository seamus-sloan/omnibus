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
#   MARKETING_VERSION  optional CFBundleShortVersionString (user-visible version,
#                      e.g. 1.4.2). Left as dx bundled it (from Cargo.toml) when
#                      unset; the workflow resolves it from the latest release tag.
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
# CFBundleShortVersionString is the user-visible "marketing" version. dx stamps
# it from mobile/Cargo.toml (0.1.0); override it here so the TestFlight build's
# version tracks the server's release tag. Same numeric/dot-separated guard as
# BUILD_NUMBER since it's interpolated into a PlistBuddy command.
if [ -n "${MARKETING_VERSION:-}" ]; then
  case "$MARKETING_VERSION" in
    *[!0-9.]*) die "MARKETING_VERSION must be numeric/dot-separated, got: '$MARKETING_VERSION'" ;;
  esac
  plist_set CFBundleShortVersionString string "$MARKETING_VERSION"
fi
# Floor at iOS 14 — the minimum for the storyboard-free UILaunchScreen the
# iPad build needs below. Set unconditionally so it matches the compile-time
# deployment target the workflow pins.
if "$plistbuddy" -c "Print :MinimumOSVersion" "$info_plist" >/dev/null 2>&1; then
  "$plistbuddy" -c "Set :MinimumOSVersion 14.0" "$info_plist"
else
  "$plistbuddy" -c "Add :MinimumOSVersion string 14.0" "$info_plist"
fi

# dx (unlike Xcode) omits the build-provenance keys that App Store validation
# reads to identify the toolchain; without them altool rejects with
# "Unsupported SDK or Xcode version". Derive them from the active Xcode/SDK so
# they truthfully describe whatever toolchain built the .app.
sdk_version="$(xcrun --sdk iphoneos --show-sdk-version)"
sdk_build="$(xcrun --sdk iphoneos --show-sdk-build-version)"
xcode_ver="$(xcodebuild -version | awk '/^Xcode/ { print $2 }')"
xcode_build="$(xcodebuild -version | awk '/^Build version/ { print $3 }')"
# DTXcode packs the version as 2-digit-major + minor + patch (26.4.1 -> "2641").
dtxcode="$(printf '%s' "$xcode_ver" | awk -F. '{ printf "%02d%d%d", $1, ($2 == "" ? 0 : $2), ($3 == "" ? 0 : $3) }')"
plist_set DTPlatformVersion string "$sdk_version"
plist_set DTSDKName string "iphoneos${sdk_version}"
plist_set DTSDKBuild string "$sdk_build"
plist_set DTPlatformBuild string "$sdk_build"
plist_set DTXcode string "$dtxcode"
plist_set DTXcodeBuild string "$xcode_build"
plist_set DTCompiler string "com.apple.compilers.llvm.clang.1_0"
plist_set BuildMachineOSBuild string "$(sw_vers -buildVersion)"

# App icon: dx bundles none, so validation fails with missing CFBundleIconName
# and missing icon files. Compile the asset catalog with actool (renders every
# iPhone/iPad size from the 1024 source into Assets.car + loose PNGs) and merge
# its icon keys. CFBundleIconName must also exist at the top level.
icon_xcassets="${ICON_XCASSETS:-mobile/assets/Assets.xcassets}"
[ -d "$icon_xcassets" ] || die "asset catalog not found: $icon_xcassets"
icon_partial="$(mktemp)"
xcrun actool "$icon_xcassets" \
  --compile "$app_dir" \
  --app-icon AppIcon \
  --platform iphoneos \
  --minimum-deployment-target 14.0 \
  --target-device iphone --target-device ipad \
  --output-partial-info-plist "$icon_partial" \
  --output-format human-readable-text >/dev/null || die "actool failed to compile app icon"
"$plistbuddy" -c "Merge $icon_partial" "$info_plist"
rm -f "$icon_partial"
plist_set CFBundleIconName string AppIcon

# dx marks the app for iPhone and iPad. App Store validation needs a single
# CFBundleSupportedPlatforms value, and iPad multitasking needs a launch
# screen — the empty UILaunchScreen dict is the storyboard-free default.
"$plistbuddy" -c "Delete :CFBundleSupportedPlatforms" "$info_plist" 2>/dev/null || true
"$plistbuddy" -c "Add :CFBundleSupportedPlatforms array" "$info_plist"
"$plistbuddy" -c "Add :CFBundleSupportedPlatforms:0 string iPhoneOS" "$info_plist"
"$plistbuddy" -c "Print :UILaunchScreen" "$info_plist" >/dev/null 2>&1 \
  || "$plistbuddy" -c "Add :UILaunchScreen dict" "$info_plist"

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
