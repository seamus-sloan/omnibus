# F1.12 — Browse authors & series

**Phase 1 · Browse & discovery** · **Priority:** P3

Dedicated `/authors` and `/series` index pages that list every author and every series in the library as browsable, sortable surfaces.

## Objective

Two new routes — `/authors` and `/series` — that serve as entry points into the taxonomy. The Authors index lists every author with their book count, sortable by name or book count, with search/filter. The Series index lists every series with book count and completion status. Both link through to the per-author and per-series detail pages from [F1.8](1-8-discovery-pages.md).

Today, the only way to reach an author page is from a book detail breadcrumb or a search result. These index pages make "browse all authors" and "browse all series" first-class navigation destinations, reachable from the top nav or sidebar.

## User / business value

Unblocks:
- **Navigation completeness** — without index pages, the author/series detail pages from F1.8 are dead ends reachable only via deep links. The index pages close the navigation loop.

## Technical considerations

- **Authors index (`/authors`):** grid or list of author cards. Each card shows: author name, book count, optional photo ([F1.11](1-11-author-profiles.md) if landed, else initial-letter avatar), accent color from their most-read book. Sortable by name (A–Z) or book count (desc). Filterable via a search input (client-side filter for libraries under 1k authors).
- **Series index (`/series`):** grid or list of series cards. Each card shows: series name, book count, completion fraction (e.g. "3 of 5"), fan-stack of covers (first 3–4 books), primary author name. Sortable by name or book count.
- **New query functions:** `list_authors(pool) -> Vec<AuthorSummary>` and `list_series(pool) -> Vec<SeriesSummary>`. Lightweight — just name + count + optional accent, no full book metadata.
- **New shared types:** `AuthorSummary { id, name, sort, book_count, accent }` and `SeriesSummary { id, name, sort, book_count, primary_author }`.
- **New RPC + REST endpoints:** `GET /api/rpc/authors`, `GET /api/rpc/series-list`, plus matching `/api/authors` and `/api/series` REST routes for mobile.
- **Nav integration:** add "Authors" and "Series" links to the top nav or as sub-items under a "Browse" dropdown. Exact placement is a design question.
- **Observability:** page-load timing for the index queries. Should be <50ms for libraries under 10k books.

## Dependencies

- [F1.8 Discovery pages](1-8-discovery-pages.md) — the per-author and per-series detail pages must exist for the index to link into.

## Risks

- **Large libraries** — a library with 5k+ unique authors could make the authors index slow to render if we load all cards at once. Mitigation: virtual scrolling or pagination. For v1, a simple full list is fine — revisit if performance data says otherwise.

## Open questions

**Resolved:**

(None yet.)

**Unresolved:**

- **Card vs list layout** — should the index pages use a card grid (like the library cover grid) or a dense list (like the library table view)? Probably both, togglable via the existing ViewMode preference.
- **Nav placement** — top-level nav items ("Authors", "Series") or nested under a "Browse" menu? Depends on nav redesign timeline.
- **Series completion** — "3 of 5" requires knowing the total planned book count for a series, which isn't in our metadata. Options: derive from max series_index, allow admin override, or just show "3 books" without a denominator.

## TODOs

### Shared types for summaries

**What:** Add `AuthorSummary` and `SeriesSummary` to `shared/src/lib.rs`.

**Why:** Lightweight types for the index pages — no full book metadata, just name + count + accent.

**Context:** Distinct from `AuthorDetail` / `SeriesDetail` (which carry full book lists). These are for the index view only.

**Effort:** S
**Priority:** P0
**Depends on:** None.

### Query functions for index pages

**What:** `list_authors(pool)` and `list_series(pool)` in `db/src/queries.rs`.

**Why:** Aggregated queries returning name + count + optional accent for all authors/series.

**Context:** Simple GROUP BY queries with COUNT. Author accent derived from the most-covered book's accent_color. Series primary_author from the first book's first author.

**Effort:** S
**Priority:** P0
**Depends on:** Shared types for summaries.

### Authors index page

**What:** `/authors` route + `AuthorsIndexPage` component.

**Why:** Browse-all entry point for author discovery.

**Context:** Reuses Atrium card/grid primitives. Shows author name, book count, avatar (letter or photo if F1.11 landed). Sortable, filterable. Links to `/authors/:id`.

**Effort:** M
**Priority:** P1
**Depends on:** Query functions for index pages.

### Series index page

**What:** `/series` route + `SeriesIndexPage` component.

**Why:** Browse-all entry point for series discovery.

**Context:** Reuses Atrium card/grid primitives. Shows series name, book count, cover fan (first 3–4 covers stacked), primary author. Links to `/series/:id`.

**Effort:** M
**Priority:** P1
**Depends on:** Query functions for index pages.

### Playwright coverage

**What:** E2E specs for both index pages.

**Why:** Verify navigation loop: index → detail → back.

**Context:** Use existing fixture data. Assert page renders, sort toggles work, clicking a card navigates to the detail page.

**Effort:** S
**Priority:** P2
**Depends on:** Authors index page, Series index page.

## Status

Queued. Blocked on [F1.8 Discovery pages](1-8-discovery-pages.md).

---

[← Back to roadmap summary](0-0-summary.md)
