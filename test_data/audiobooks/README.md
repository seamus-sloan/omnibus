# Playwright audiobook fixtures

This directory holds the audiobooks the Playwright listen-page spec seeds
against. The test-only `seedAudiobookLibrary()` helper points the running
server at this absolute path and waits for the indexer to surface the expected
number of books listed in
`ui_tests/playwright/tests/fixtures/audiobooks.ts`.

## Contents

```
public_domain/
  Arthur Conan Doyle/
    A Womans Love.m4b          — single-file M4B (converted from LibriVox MP3s)
  James Whitcomb Riley/
    A Song of Long Ago/
      01_a_song_of_long_ago.mp3  — multi-file MP3 folder (3 tracks)
      02_a_song_of_long_ago.mp3
      03_a_song_of_long_ago.mp3
```

## Sources

All files are public domain recordings from [LibriVox](https://librivox.org/):

- **A Song of Long Ago** — James Whitcomb Riley, read by various readers.
  [archive.org](https://archive.org/details/songoflongago_2605.poem_librivox)
- **A Woman's Love** — Sir Arthur Conan Doyle, read by various readers.
  [archive.org](https://archive.org/details/womanslove_2605.poem_librivox)

The M4B was produced by concatenating three source MP3 tracks and
re-encoding to AAC via ffmpeg — see `ui_tests/playwright/tools/make_audiobook.sh`.

## Regenerating

```bash
cd ui_tests/playwright
bash tools/make_audiobook.sh
```

The script requires `ffmpeg` on `$PATH` and the source LibriVox MP3s in
`~/Downloads/`. See the script header for the exact archive.org URLs.
