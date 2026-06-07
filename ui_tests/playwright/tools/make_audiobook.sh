#!/usr/bin/env bash
# Reproduce the audiobook fixtures under test_data/audiobooks/public_domain/.
#
# Sources (public domain, LibriVox):
#   https://archive.org/details/songoflongago_2605.poem_librivox
#   https://archive.org/details/womanslove_2605.poem_librivox
#
# Prerequisites:
#   - ffmpeg on $PATH
#   - Source MP3s in ~/Downloads/{songoflongago,womanslove}_2605.poem_librivox/
#
# Usage:
#   bash ui_tests/playwright/tools/make_audiobook.sh
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="$REPO/test_data/audiobooks/public_domain"
DL="$HOME/Downloads"

SONG_SRC="$DL/songoflongago_2605.poem_librivox"
WOMAN_SRC="$DL/womanslove_2605.poem_librivox"

for d in "$SONG_SRC" "$WOMAN_SRC"; do
  if [ ! -d "$d" ]; then
    echo "ERROR: missing source directory: $d" >&2
    echo "Download from archive.org first." >&2
    exit 1
  fi
done

command -v ffmpeg >/dev/null 2>&1 || { echo "ERROR: ffmpeg not found" >&2; exit 1; }

# --- MP3 book: A Song of Long Ago (3 tracks) ---
MP3_DIR="$OUT/James Whitcomb Riley/A Song of Long Ago"
mkdir -p "$MP3_DIR"
cp "$SONG_SRC/songoflongago_riley_al_64kb.mp3"  "$MP3_DIR/01_a_song_of_long_ago.mp3"
cp "$SONG_SRC/songoflongago_riley_bk_64kb.mp3"  "$MP3_DIR/02_a_song_of_long_ago.mp3"
cp "$SONG_SRC/songoflongago_riley_bwc_64kb.mp3" "$MP3_DIR/03_a_song_of_long_ago.mp3"
echo "MP3 book: A Song of Long Ago (3 tracks)"

# --- M4B book: A Woman's Love (concatenated from 3 MP3 tracks) ---
M4B_DIR="$OUT/Arthur Conan Doyle"
mkdir -p "$M4B_DIR"

CONCAT_FILE="$(mktemp)"
cat > "$CONCAT_FILE" <<EOF
file '$WOMAN_SRC/womanslove_doyle_ac_64kb.mp3'
file '$WOMAN_SRC/womanslove_doyle_al_64kb.mp3'
file '$WOMAN_SRC/womanslove_doyle_bk_64kb.mp3'
EOF

ffmpeg -y -f concat -safe 0 -i "$CONCAT_FILE" \
  -c:a aac -b:a 64k -f ipod \
  -metadata title="A Woman's Love" \
  -metadata artist="Sir Arthur Conan Doyle" \
  -metadata album="A Woman's Love" \
  -metadata genre="speech" \
  "$M4B_DIR/A Womans Love.m4b" 2>/dev/null

rm -f "$CONCAT_FILE"
echo "M4B book: A Woman's Love (1 file from 3 concatenated tracks)"

echo "Done — $(find "$OUT" -type f | wc -l | tr -d ' ') files in $OUT"
