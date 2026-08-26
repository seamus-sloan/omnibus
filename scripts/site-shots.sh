#!/usr/bin/env bash
# Re-encode the marketing site's screenshots.
#
# The PNGs are exported at 2x from the Claude Design "Omnibus" project (file
# `Omnibus - Site.html`); that project is their source of truth and they are
# not committed. Only the WebP output under site/src/shots/ is, so a clone
# needs no encoder to build the site.
#
#   ./scripts/site-shots.sh ~/Downloads              # -> site/src/shots/dark/
#   ./scripts/site-shots.sh ~/Downloads sepia        # -> site/src/shots/sepia/
#
# Output is namespaced by colour direction so a second set can sit beside the
# first without renaming either. Names must match the <img src> values in
# site/src/index.html.
set -euo pipefail

SRC="${1:-}"
VARIANT="${2:-dark}"
[ -n "$SRC" ] && [ -d "$SRC" ] || { echo "usage: $0 <dir-of-omnibus-*.png> [variant]" >&2; exit 2; }
case "$VARIANT" in
  */*|"") echo "variant must be a plain directory name, e.g. dark or sepia" >&2; exit 2 ;;
esac

command -v cwebp >/dev/null || { echo "cwebp not found — nix shell nixpkgs#libwebp" >&2; exit 1; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/site/src/shots/$VARIANT"
mkdir -p "$OUT"

shopt -s nullglob
found=0
for f in "$SRC"/omnibus-*.png; do
  found=1
  b="$(basename "$f" .png)"; b="${b%@2x}"
  # Desktop screens display at most ~1440 CSS px, so 1920 stays crisp on a 2x
  # display without shipping the full 2884px export. Phone frames cap at 820.
  case "$b" in
    *ios*|*android*|*checkin-scan*|*wishlist*) W=820 ;;
    *)                                         W=1920 ;;
  esac
  cwebp -quiet -q 78 -resize "$W" 0 -m 6 "$f" -o "$OUT/$b.webp"
  printf '%-34s %8s -> %8s\n' "$b.webp" "$(du -h "$f" | cut -f1)" "$(du -h "$OUT/$b.webp" | cut -f1)"
done

[ "$found" = 1 ] || { echo "no omnibus-*.png in $SRC" >&2; exit 1; }
echo "total ($VARIANT): $(du -sh "$OUT" | cut -f1)"
