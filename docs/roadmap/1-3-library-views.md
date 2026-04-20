# F1.3 — Library views

**Phase 1 · Browse & discovery** · **Priority:** P1

Table view + cover grid, view preference persisted per library.

## Objective

Two first-class list views (dense table, cover grid) with a toggle that persists per-library per-user. Sort and filter happen in-memory over a hydrated signal for libraries under ~10k books.

## User / business value

The primary Omnibus surface after auth. The dense table mirrors the Calibre desktop experience power users expect; the cover grid is the "home page of books I'm in the mood for" view that differentiates us.

## Technical considerations

- **Keyset pagination** on `(sort, id)` — never `OFFSET`. Calibre-Web's `OFFSET`-based pagination is its large-library killer (see [calibre-inspection §3](../calibre-inspection/3-performance-pain-points.md)).
- **Client-side sort** on already-hydrated lists for libraries ≤10k books. Dioxus signals re-filter in-memory; network round-trip only crosses the page boundary. This is the biggest perceived-speed win over Calibre-Web ([recommendation #12](../calibre-inspection/7-recommendations.md)).
- Single JOIN query with `GROUP_CONCAT(authors)` / `GROUP_CONCAT(tags)` into a flat `BookListItem` DTO — never iterate relationships in a render path.
- `srcset` from [F1.2](1-2-thumbnails.md) for cover-grid sizing.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) for normalized m2m relations.
- [F1.2 Thumbnail pipeline](1-2-thumbnails.md).

## Changes from v1

- v1 §4 implied offset pagination; spec now requires keyset.
- Client-side sort was not called out in v1; it's the largest measurable UX delta.

---

[← Back to roadmap summary](0-0-summary.md)
