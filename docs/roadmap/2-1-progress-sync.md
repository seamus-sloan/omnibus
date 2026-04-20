# F2.1 — Progress sync service

**Phase 2 · Reading & listening** · **Priority:** P0

Single unified progress endpoint serving epub and audiobook clients, web and mobile.

## Objective

One `POST /api/progress` endpoint accepting a discriminated payload (`{ epub_cfi }` or `{ audio_position_seconds }`). One `reading_progress` table with a `format` discriminator column. Clients (web reader, web audio, mobile) all call the same route.

## User / business value

Building reader ([F2.2](2-2-epub-reader.md)) and audio player ([F2.3](2-3-audiobook-player.md)) against separate sync mechanisms guarantees divergence. One service means: same debounce strategy, same offline-queue semantics, same last-write-wins reconciliation, and mobile gets it for free.

## Technical considerations

- Discriminated payload typed at the API boundary; server fans out to per-format write paths internally.
- Last-write-wins on `(user_id, book_id, format)`; conflicts reconcile client-side so we don't need CRDT infrastructure.
- Client-side debounce (~5s for reading, ~15s for audio) and offline queue in IndexedDB (web) / SQLite (mobile).
- Endpoint returns the server's authoritative position so a newly opened client syncs forward.
- **Bookmarks as their own table**, not a JSON blob on `users`: `bookmarks(id, user_id, book_id, position, title, created_at)` with `(user_id, book_id)` index. ABS stores bookmarks as a JSON array on the user row and rewrites the whole row on every add/remove — no FK to books, no concurrent-write safety ([ABS recommendation #6](../audiobookshelf-inspection/7-recommendations.md), [User.js](https://github.com/advplyr/audiobookshelf/blob/master/server/models/User.js)).
- **`listening_sessions` table** (one row per playback session: `user_id`, `book_id`, `started_at`, `ended_at`, `seconds_listened`, `device_id`) to feed stats/year-in-review without re-deriving from progress writes. Mirrors ABS's [PlaybackSession model](https://github.com/advplyr/audiobookshelf/blob/master/server/models/PlaybackSession.js). Mobile posts batched sessions on reconnect via a `POST /api/progress/sessions` companion route. Pair with an analogous `reading_sessions` table for the epub reader so stats can aggregate audiobooks listened, epubs read, and (future) journal entries across week / month / 6-month / year / lifetime windows — designing the session shape now is what makes that feature cheap later.

## Dependencies

- [F0.3 Auth](0-3-auth.md) — needs `user_id`.

## Risks

- Dioxus fullstack + mobile authentication paths need to agree on how bearer tokens flow into `POST /api/progress`. Prototype in F0.3.

---

[← Back to roadmap summary](0-0-summary.md)
