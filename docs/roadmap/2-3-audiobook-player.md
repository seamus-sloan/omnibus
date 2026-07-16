# F2.3 — In-browser audiobook player

**Phase 2 · Reading & listening** · **Priority:** P1

HTML5 audio with chapter navigation, playback speed, and progress sync.

## Objective

Play m4a/m4b audiobooks in the browser with a chapter list, scrub bar, playback speed (0.5×–2×), and position persisted via [F2.1](2-1-progress-sync.md).

## User / business value

Audiobook support is the feature AudioBookShelf users currently leave for — lacking it would make Omnibus "just another Calibre-Web." Ships alongside the epub reader so the library is meaningful for both mediums from Phase 2.

## Technical considerations

- HTML5 `<audio>` is sufficient for direct-play (m4a/m4b/mp3). Add `hls.js` only when we need to transcode FLAC or multi-file m4b stitching — follow the AudioBookShelf pattern without copying the per-session ffmpeg ([ABS pain points](../audiobookshelf-inspection/3-performance-pain-points.md)).
- **Shared HLS segment cache** keyed on `(book_id, codec_profile, segment_index)`, stored under `<data_dir>/hls/<book_id>/<profile>/seg-NNN.ts`. One `tokio::sync::Mutex` per `(book, profile)` so two users on the same audiobook trigger one transcode, not two ([ABS recommendation #4](../audiobookshelf-inspection/7-recommendations.md)).
- Chapters come from the m4b container via `mp4ameta` for native atoms; fall back to ffprobe for mp3/flac embedded chapters. Persist into a `file_chapters` table keyed on `book_file_id`, populated during scan via [F0.5 background worker](0-5-background-worker.md). Use `JoinSet` + `Semaphore(num_cpus)` — ABS's serial ffprobe loop is the single biggest scanner bottleneck.
- Position is seconds (float); writes through F2.1 on debounce + on pause/unload.
- Playback speed is stored separately from progress, keyed by authenticated user
  and durable book UUID, and synced across devices. An account-scoped local
  value is only a fallback and seeds the server when no preference exists yet.
- Metadata embed tool (write ID3/m4b tags back) is deferred — admin-only feature for a later phase; see ABS's `/api/tools/item/:id/embed-metadata`.

## Status

**F2.3 foundation (PR #327):** single-file audiobook indexing + basic HTML5 `<audio>` player. Merged.

**F2.3 HLS pipeline (PR #331):** multi-file audiobook grouping (`group_into_books`), Phase B
multi-part tag parse (`parse_groups`), `book_file_parts` schema, ffmpeg-backed HLS transcode cache
(`db::hls`), `Task::HlsTranscode` worker variant, `/playlist.m3u8` + `/segments/` + `/status`
routes, hls.js 1.6.16 vendored, "Preparing…" overlay. Merged.

**Next increment:** Chapter-list UI — extract chapter atoms from m4b containers (via `mp4ameta`)
and embedded ID3 chapters from mp3s (via lofty), persist into a `file_chapters` table keyed on
`book_file_id`, and surface a scrollable chapter sidebar in the listen page. The `book_file_parts`
schema already provides a natural anchor for chapter data.

## Dependencies

- [F2.1 Progress sync service](2-1-progress-sync.md).
- [F0.1 Schema refactor](0-1-schema-refactor.md) for `book_files` + `file_chapters`.

---

[← Back to roadmap summary](0-0-summary.md)
