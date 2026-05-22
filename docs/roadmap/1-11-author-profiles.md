# F1.11 — Author profile pictures

**Phase 1 · Browse & discovery** · **Priority:** P3

Cascading author photo resolution so author pages show a real profile picture instead of a typographic initial-letter avatar.

## Objective

Add a multi-source cascading resolver for author profile photos. When an author page loads, the system checks (in order): manual admin upload → Open Library API → Wikidata/Wikipedia → letter avatar fallback. Resolved images are cached locally so subsequent page loads are instant.

## User / business value

Unblocks:
- **Author page visual identity** — the F1.8 author page currently shows a serif-initial circle avatar. A real photo makes the page feel polished and recognizable, especially for well-known authors.

## Technical considerations

- **Resolution cascade:** `manual upload` → `Open Library covers API` → `Wikidata/Wikipedia Commons` → `letter avatar` (current fallback).
- **Schema:** new `author_photos` table: `author_id INTEGER PRIMARY KEY, source TEXT, url TEXT, blob BLOB, mime TEXT, fetched_at TEXT`.
- **Resolver worker task:** `Task::ResolveAuthorPhoto { author_id }` — runs the cascade, stores the first hit, skips if a manual override exists.
- **API:** `GET /api/authors/:id/photo` → serves the cached photo (or 404 if none resolved). `PUT /api/authors/:id/photo` (admin) → manual upload override.
- **Frontend:** `AuthorPage` hero section checks for a photo URL before falling back to the letter avatar.
- **Observability:** log resolver hits/misses per source. Track average resolution time.

## Dependencies

- [F1.8 Discovery pages](1-8-discovery-pages.md) — author page must exist for the photo to render on.

## Risks

- **External API reliability** — Open Library and Wikidata may be slow or return low-quality images. Mitigation: timeout + fallback chain.
- **Name matching** — author name in the EPUB may not match the canonical name in external APIs. Mitigation: try `file_as` (surname-first) as a secondary lookup.

## Open questions

**Resolved:**

- **Cascade order** — Manual → Open Library → Wikidata → Letter. Decided by user preference.

**Unresolved:**

- **Cache invalidation** — how long before we re-check external sources for an author whose photo resolved to "letter"? Weekly? Never until admin action?
- **Image sizing** — should we store a single size or generate thumbnails (sm/md/lg) like book covers?

## TODOs

### Schema and migration

**What:** Add `author_photos` table with `author_id`, `source`, `url`, `blob`, `mime`, `fetched_at` columns.

**Why:** Persistent cache for resolved photos so the cascade doesn't re-run on every page load.

**Context:** FK to `authors.id`. `source` is one of `manual`, `openlibrary`, `wikidata`, `letter`.

**Effort:** S
**Priority:** P0
**Depends on:** None.

### Resolver worker task

**What:** `Task::ResolveAuthorPhoto` that runs the cascade for a given author.

**Why:** Background resolution keeps the author page fast — it shows the letter avatar immediately and upgrades to the real photo once resolved.

**Context:** Runs on first view of an author page (if no photo cached) or on admin trigger. Cascade order: check `author_photos` for manual → try Open Library → try Wikidata → store "letter" as negative cache.

**Effort:** M
**Priority:** P1
**Depends on:** Schema and migration.

### API endpoints

**What:** `GET /api/authors/:id/photo` and `PUT /api/authors/:id/photo` (admin upload).

**Why:** Frontend needs a URL to fetch the photo; admin needs an upload path for manual overrides.

**Context:** GET returns the cached blob with appropriate `Content-Type`. PUT accepts multipart upload and sets `source = 'manual'`.

**Effort:** S
**Priority:** P1
**Depends on:** Schema and migration.

### Frontend integration

**What:** Update `AuthorPage` hero to show the photo when available.

**Why:** Visual upgrade from the letter avatar.

**Context:** Check `/api/authors/:id/photo` — on 200 render `<img>`, on 404 fall back to the existing `.disc-avatar` letter circle.

**Effort:** S
**Priority:** P2
**Depends on:** API endpoints.

## Status

Queued. Blocked on [F1.8 Discovery pages](1-8-discovery-pages.md).

---

[← Back to roadmap summary](0-0-summary.md)
