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

## Dependencies

- [F0.3 Auth](0-3-auth.md) — needs `user_id`.

## Risks

- Dioxus fullstack + mobile authentication paths need to agree on how bearer tokens flow into `POST /api/progress`. Prototype in F0.3.

---

[← Back to roadmap summary](0-0-summary.md)
