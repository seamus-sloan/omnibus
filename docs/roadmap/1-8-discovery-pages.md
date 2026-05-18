# F1.8 — Discovery pages

**Phase 1 · Browse & discovery** · **Priority:** P2

Dedicated Author, Series, and Tag-cloud landing pages so taxonomy is a first-class browse surface.

## Objective

Three new routes — `/authors/:slug`, `/series/:slug`, `/tags` — backed by the normalized author/series/tag relations from [F0.1](0-1-schema-refactor.md). Each Author page lists every book by that author with the existing cover-grid component, a short bio header, and "Also published as" aliases. Each Series page lists books in reading order with progress indicators. The Tag cloud is an overview surface that visualizes tag weights (count) and supports faceted intersect (`tag:fantasy + tag:dark-academia`).

## User / business value

Today taxonomy is a filter — applied from the toolbar, leaves no shareable URL. Discovery pages make "the library by Susanna Clarke" a real place users can bookmark, link to, and visit from a book-detail breadcrumb. Tag cloud surfaces the long tail of a curated library in a way that pure search can't.

## Technical considerations

- All three pages reuse the Atrium `Cover` + `SectionHead` primitives from [F1.7](1-7-atrium-design-system.md).
- New server functions: `rpc_get_author(slug)`, `rpc_get_series(slug)`, `rpc_get_tag_cloud()` returning aggregated counts. All read-only — no schema changes.
- Tag cloud weights computed in SQL via `SELECT tag, COUNT(*) FROM book_tags GROUP BY tag`. Cache result in-process for 5 min (tags rarely change between scans).
- Slug resolution at the DB layer: an `authors.slug` column (already proposed in [F0.1](0-1-schema-refactor.md)) is the join key. If F0.1 hasn't landed it, this initiative depends on adding it.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) — normalized authors / series / tags with slug columns.
- [F1.7 Atrium design system](1-7-atrium-design-system.md) — visual primitives.

## Acceptance criteria

- `/authors/susanna-clarke` renders the Atrium author page with every book by Susanna Clarke.
- `/series/kingkiller-chronicle` renders the series page in reading order with per-book progress.
- `/tags` renders the tag cloud; clicking a tag deep-links to `/?tags=fantasy`.
- Playwright covers the three flows.

## Related

- [Atrium design doc](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)
