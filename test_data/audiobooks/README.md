# Playwright audiobook fixtures

This directory holds the audiobooks the Playwright listen-page spec seeds
against. The test-only `seedAudiobookLibrary()` helper points the running
server at this absolute path and waits for the indexer to surface the
expected number of books listed in
`ui_tests/playwright/tests/fixtures/audiobooks.ts`.

## Contents

```
generated/          — synthetic silent MP3s produced by tools/make_audiobook.ts (committed)
public_domain/      — real audiobooks from LibriVox (committed)
```

The seeder points the server at `test_data/audiobooks/` (this directory).
The scanner recurses, so both subdirectories load in a single seed call.

## Synthetic vs. public-domain

The synthetic MP3s cover both grouping patterns the indexer handles
(single-file book vs. multi-part mp3 folder) with deterministic metadata
and embedded 1×1 PNG cover images, so the cover extraction + thumbnail
pipeline is exercised. The public-domain files exercise the metadata
parser against real-world tags we don't control, and add M4B format
coverage the synthetic generator can't produce without ffmpeg.

## Single source of truth for expected metadata

`ui_tests/playwright/tests/fixtures/audiobooks.ts` exports
`AUDIOBOOK_BOOKS`, the table the spec asserts against. The generator
inputs in `tools/make_audiobook.ts` and that table must stay in sync.

## Regenerating

The synthetic fixtures are deterministic (re-running produces
byte-identical output):

```bash
cd ui_tests/playwright
npx tsx tools/make_audiobook.ts
```

## Public-domain M4B provenance

The M4B (`Arthur Conan Doyle/A Womans Love.m4b`) was converted from
three LibriVox MP3 tracks via ffmpeg and is committed as-is:

```bash
# Sources: https://archive.org/details/womanslove_2605.poem_librivox
ffmpeg -f concat -safe 0 -i <(printf "file '%s'\n" \
  womanslove_doyle_ac_64kb.mp3 \
  womanslove_doyle_al_64kb.mp3 \
  womanslove_doyle_bk_64kb.mp3) \
  -c:a aac -b:a 64k -f ipod \
  -metadata title="A Woman's Love" \
  -metadata artist="Sir Arthur Conan Doyle" \
  -metadata album="A Woman's Love" \
  -metadata genre="speech" \
  "A Womans Love.m4b"
```

The MP3 folder (`James Whitcomb Riley/A Song of Long Ago/`) contains
unmodified tracks from
[archive.org](https://archive.org/details/songoflongago_2605.poem_librivox).
