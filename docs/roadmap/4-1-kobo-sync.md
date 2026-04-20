# F4.1 — Native Kobo sync

**Phase 4 · Device sync** · **Priority:** P1

Implement the `/kobo/v1/*` protocol natively, including EPUB → KEPUB conversion via kepubify.

## Objective

Implement `/kobo/v1/library/sync`, `/kobo/v1/library/<uuid>/metadata`, `/kobo/v1/library/<uuid>/state`, `/kobo/v1/library/tags`, and `download/*` endpoints. See [calibre-inspection §6](../calibre-inspection/6-api-surface.md) for the route inventory.

## User / business value

Closes [gap G9](0-0-summary.md#gaps). Native Kobo sync is a **far superior UX** to OPDS on Kobo devices: background sync, reading-state round-trip, shelves-as-tags, no manual re-browse. Calibre-Web's implementation is the de-facto reference — users arriving from Calibre-Web expect this as table stakes. Promoted ahead of OPDS in the phasing because it's the better UX for the platform that matters most.

## Sub-scope: EPUB → KEPUB via kepubify

Kobo devices render plain EPUB but with measurably slower page turns than KEPUB; shipping the plain file is leaving UX on the table.

- Detect [`kepubify`](https://github.com/pgaskin/kepubify) on `PATH` at startup.
- On first Kobo download of each book, run `kepubify` via the [F0.5 worker](0-5-background-worker.md); cache output at `<data_dir>/kepub/<book_id>.kepub.epub`; serve that on subsequent requests.
- Invalidate cache on `book.last_modified` bump.
- If kepubify is absent, fall back to plain EPUB with a one-time admin warning in the log.
- Bundle kepubify in the Nix dev shell and in release images; keep it optional at runtime for users who build from source.
- See [assumption A6](0-0-summary.md#assumptions) on kepubify's stability.

## Technical considerations

- **Stream the sync response** via `axum::body::StreamBody` — **do not** copy Calibre-Web's `SYNC_ITEM_LIMIT=100` cap. See [calibre-inspection §3](../calibre-inspection/3-performance-pain-points.md) and [recommendation #13](../calibre-inspection/7-recommendations.md).
- `books.uuid` column needs an index — Kobo identifies books by uuid, not by integer id.
- Sync tokens stored in a `kobo_sync_tokens` table keyed on user.
- Reading state (bookmarks + statistics) flows through [F2.1](2-1-progress-sync.md) internally — Kobo endpoints translate.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) (uuid index).
- [F0.3 Auth](0-3-auth.md).
- [F0.5 Background worker](0-5-background-worker.md) (kepubify jobs).
- [F2.1 Progress sync service](2-1-progress-sync.md) (reading-state translation).

## Risks

- Kobo protocol is undocumented. Calibre-Web's implementation is our reference; scope for v1.0 is "parity with Calibre-Web minus the 100-item cap."
- kepubify absence degrades gracefully; not a blocker.

---

[← Back to roadmap summary](0-0-summary.md)
