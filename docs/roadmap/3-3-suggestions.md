# F3.3 — Suggestions

**Phase 3 · Personalization** · **Priority:** P3

"More like this" on the book detail page, powered by local signals + OpenLibrary.

## Objective

Below the metadata on a book detail page, show: (a) other books by the same authors, (b) other books in the same series, (c) tag-overlap matches from the local library, (d) OpenLibrary "readers also enjoyed" data.

## User / business value

Discovery within the library. Cheap to ship for (a)-(c) because the data is already in-DB; OpenLibrary adds the "what else does this look like" dimension without per-user API keys.

## Technical considerations

- Local signals are pure SQL; no ML, no embeddings.
- OpenLibrary calls cached by ISBN/OLID, TTL 30d, served through the [F0.5 worker](0-5-background-worker.md) so the detail page doesn't block on a network call.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).

## Changes from v1

- **Hardcover dropped.** v1 listed Hardcover as a source; adding it requires per-user API-key management, rate-limit handling, and caching complexity not justified until users ask for it.

---

[← Back to roadmap summary](0-0-summary.md)
