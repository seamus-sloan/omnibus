# F2.3 — In-browser audiobook player

**Phase 2 · Reading & listening** · **Priority:** P1

HTML5 audio with chapter navigation, playback speed, and progress sync.

## Objective

Play m4a/m4b audiobooks in the browser with a chapter list, scrub bar, playback speed (0.5×–2×), and position persisted via [F2.1](2-1-progress-sync.md).

## User / business value

Audiobook support is the feature AudioBookShelf users currently leave for — lacking it would make Omnibus "just another Calibre-Web." Ships alongside the epub reader so the library is meaningful for both mediums from Phase 2.

## Technical considerations

- HTML5 `<audio>` is sufficient — no need for SoundManager or third-party players.
- Chapters come from the m4b container via `mp4ameta`. Persist into a `file_chapters` table keyed on `book_file_id`, populated during scan.
- Position is seconds (float); writes through F2.1 on debounce + on pause/unload.
- Playback speed lives in localStorage per-user — not synced (device-specific preference).

## Dependencies

- [F2.1 Progress sync service](2-1-progress-sync.md).
- [F0.1 Schema refactor](0-1-schema-refactor.md) for `book_files` + `file_chapters`.

---

[← Back to roadmap summary](0-0-summary.md)
