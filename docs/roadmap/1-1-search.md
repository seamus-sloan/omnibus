# F1.1 — Search

**Phase 1 · Browse & discovery** · **Priority:** P0

Single search box on every page, querying the FTS5 index from [F0.4](0-4-fts5.md).

## Objective

A site-wide search box that queries `books_fts`, returns bm25-ranked results, and supports simple field facets (`author:`, `series:`, `tag:`).

## User / business value

The feature Calibre-Web users most complain about (see [calibre-inspection §2](../calibre-inspection/2-feature-inventory.md) — Calibre-Web's default search is `LIKE '%q%'`, no tokenization, no ranking). Closes [gap G1](0-0-summary.md#gaps). Cheap to ship on top of F0.4.

## Technical considerations

- `SELECT … FROM books_fts WHERE books_fts MATCH ? ORDER BY bm25(books_fts) LIMIT ?`.
- Dioxus signal-debounced input (~150ms) so each keystroke doesn't fire a query.
- Results hydrate the same `BookListItem` DTO used by browse pages — one component, two data sources.
- Facet syntax parsed client-side before query construction; unknown facets fall through as free-text terms.

## Dependencies

- [F0.4 FTS5 index](0-4-fts5.md).

## Risks

None material. FTS5 is the mature, proven path.

---

[← Back to roadmap summary](0-0-summary.md)
