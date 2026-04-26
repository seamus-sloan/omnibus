# F1.1 — Search

**Phase 1 · Browse & discovery** · **Priority:** P0

Single search box on every page, querying the FTS5 index from [F0.4](0-4-fts5.md).

## Objective

A site-wide search box that queries `books_fts`, returns bm25-ranked results, and supports simple field facets (`author:`, `series:`, `tag:`).

## User / business value

The feature Calibre-Web users most complain about (see [calibre-inspection §2](../calibre-inspection/2-feature-inventory.md) — Calibre-Web's default search is `LIKE '%q%'`, no tokenization, no ranking). Closes [gap G1](0-0-summary.md#gaps). Cheap to ship on top of F0.4.

## Technical considerations

- `SELECT … FROM books_fts WHERE books_fts MATCH ? ORDER BY bm25(books_fts) LIMIT ?`.
- Results hydrate the same `EbookMetadata` DTO used by browse pages — one component, two data sources.
- Facet syntax parsed in `build_fts_match` before MATCH construction; unknown facets fall through as free-text terms.
- No keystroke debounce — FTS5 against the local SQLite index is fast enough that snappy live-update beats batched fetches.

## Dependencies

- [F0.4 FTS5 index](0-4-fts5.md).

## Risks

None material. FTS5 is the mature, proven path.

## Status

**Shipped.** The search box lives in the top nav (web) and is hidden on `/settings`; results render in the existing landing layout via a shared `SearchQuery` signal context. Off-Landing keystrokes auto-redirect to `/`. Facet parsing supports `author:`, `series:`, `tag:` (case-insensitive prefix); unknown prefixes fall through as free-text. Mobile parity (search box in `BottomNav`) is deferred until the mobile browse story matures.

**Key landmarks:**
- [db/src/queries.rs](../../db/src/queries.rs) — `build_fts_match` + `search_books`, scoped to `{title authors series}` for free-text and column-filtered for facets, joined by explicit `AND`.
- [frontend/src/components/top_nav.rs](../../frontend/src/components/top_nav.rs) — site-wide `NavSearch` component, hidden on `Route::Settings`.
- [frontend/src/lib.rs](../../frontend/src/lib.rs) — `SearchQuery` context provider; `.top-nav .library-search` styling matches the dark Settings input look.
- [ui_tests/playwright/tests/flows/search.spec.ts](../../ui_tests/playwright/tests/flows/search.spec.ts) — covers title/author/clear, facet `author:` and `tag:`, nav-availability, and settings-absence.

---

[← Back to roadmap summary](0-0-summary.md)
