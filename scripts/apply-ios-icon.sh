#!/usr/bin/env bash
# Inject the Omnibus app icon into a dx-built iOS `.app` bundle.
#
# dx 0.7.x installs no iOS launcher icon, and `Dioxus.toml`'s `[bundle] icon`
# only feeds the desktop bundlers (upstream bug DioxusLabs/dioxus#3685). So we
# compile an asset catalog from mobile/assets/app-icon.png with `actool` and
# merge the resulting CFBundleIcons keys into the app's Info.plist — a
# post-build step you run against the `.app` dx produced.
#
# Usage:
#   scripts/apply-ios-icon.sh <path/to/App.app> [iphonesimulator|iphoneos]
#
# Re-runnable and idempotent: it overwrites any icon it previously injected.
set -euo pipefail

APP="${1:?usage: apply-ios-icon.sh <path/to/App.app> [iphonesimulator|iphoneos]}"
SDK="${2:-iphonesimulator}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICON="$REPO_ROOT/mobile/assets/app-icon.png"
PLIST="$APP/Info.plist"

[ -d "$APP" ] || { echo "error: no .app at $APP" >&2; exit 1; }
[ -f "$ICON" ] || { echo "error: missing master icon $ICON" >&2; exit 1; }
: "${DEVELOPER_DIR:=/Applications/Xcode.app/Contents/Developer}"
export DEVELOPER_DIR

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
iconset="$work/AppIcon.xcassets/AppIcon.appiconset"
mkdir -p "$iconset"
# A single 1024² universal iOS icon; actool derives every runtime size from it.
cp "$ICON" "$iconset/icon-1024.png"
printf '{ "info": { "author": "xcode", "version": 1 } }\n' > "$work/AppIcon.xcassets/Contents.json"
cat > "$iconset/Contents.json" <<'JSON'
{
  "images": [
    { "filename": "icon-1024.png", "idiom": "universal", "platform": "ios", "size": "1024x1024" }
  ],
  "info": { "author": "xcode", "version": 1 }
}
JSON

xcrun actool "$work/AppIcon.xcassets" \
  --compile "$APP" \
  --platform "$SDK" \
  --minimum-deployment-target 15.0 \
  --app-icon AppIcon \
  --output-partial-info-plist "$work/partial.plist" \
  --output-format human-readable-text \
  --errors --warnings >/dev/null

# Merge the CFBundleIcons / CFBundleIcons~ipad keys actool emitted into Info.plist.
/usr/libexec/PlistBuddy -c "Delete :CFBundleIcons" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Delete :CFBundleIcons~ipad" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Merge $work/partial.plist" "$PLIST"

echo "injected app icon into $APP (sdk=$SDK)"
