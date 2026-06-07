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

The public-domain M4B was converted from LibriVox MP3s via ffmpeg — see
`tools/make_audiobook.sh` for provenance. The M4B is committed as-is
and does not need regeneration.
