# F1.5 — Advanced search

**Phase 1 · Browse & discovery** · **Priority:** P1

A popout filter panel anchored below the nav search box that combines free-text
with structured facet filters (author, series, tag/genre, publisher, language)
sourced from the normalized taxonomy tables, plus a structured filter API the
mobile app can consume directly.

## Objective

Add a discoverable, structured complement to the [F1.1](1-1-search.md) free-text search:

- A modal that expands below the existing nav search input on demand
  (Google-expanded-search style), not a separate route.
- Multi-select facet pickers populated from the live taxonomy
  (`authors`, `series`, `tags`, `publishers`, `languages`) with
  type-ahead. Selections combine with AND across facets, OR within a facet.
- A free-text box that targets the title/authors/series scope already used
  by [F1.1](1-1-search.md)'s `search_books`.
- An "Apply" action that pushes a structured query into the URL (so the
  filtered view is shareable / back-button-correct) and re-renders the
  landing list using the existing book-row rendering of `EbookMetadata`.
- A new `GET /api/books/search` REST endpoint mirroring the Dioxus server
  function so the mobile shell can issue the same query.

The narrowest wedge ships **author, series, tag, publisher, language**
facets and the free-text box. Narrator is deferred (see Open questions).

## User / business value

Closes the residual half of [gap G1](0-0-summary.md#32-gaps): [F1.1](1-1-search.md) made search
tokenized and ranked, but the only way to filter today is the invisible
`author:` / `series:` / `tag:` facet syntax. Self-hosters migrating from
Calibre-Web expect publisher and language filters, and the population
moving from a flat library to ten-thousand-book personal collections needs
slice-and-dice that doesn't require typing a DSL.

Unblocks:

- **F3.1 Libraries with metadata filters**
  ([F3.1](3-1-libraries.md)) — saved-filter "libraries" persist the same
  rule shape this initiative defines for the URL/API contract. Building
  the rule shape once, here, prevents [F3.1](3-1-libraries.md) from
  reinventing it.
- **F4.2 OPDS 1.2 feed** ([F4.2](4-2-opds.md)) — OPDS faceted navigation
  reuses the same taxonomy queries that populate this modal.

## Technical considerations

- **Modal anchored to nav search.** Lives in
  [frontend/src/components/top_nav.rs](../../frontend/src/components/top_nav.rs)
  next to the existing `NavSearch`. Keyboard: Esc closes; Enter on the
  free-text box submits. Click-outside closes. Hidden on `/settings` and
  `/login` like the existing nav search.
- **Facet picker data source.** Add taxonomy-listing queries to
  [db/src/queries.rs](../../db/src/queries.rs):
  `list_authors_prefix(pool, q, limit)`, plus the analogous functions for
  series / tags / publishers / languages. Each returns
  `Vec<TaxonomyOption { id, name, book_count }>` so the UI can show
  result counts next to each option (Calibre-Web's faceted browse does
  this; users find it hugely useful for picking productive filters).
- **Structured filter shape — the contract.** New type in
  [shared/src/lib.rs](../../shared/src/lib.rs):
  ```rust
  #[non_exhaustive]
  pub struct AdvancedSearchQuery {
      pub q: Option<String>,                // free-text (FTS5)
      pub authors: Vec<i64>,                // OR within, AND across
      pub series: Vec<i64>,
      pub tags: Vec<i64>,                   // tags == "genre"
      pub publishers: Vec<i64>,
      pub languages: Vec<i64>,
      // pub narrators: Vec<i64>,           // reserved — see Open questions
      pub limit: Option<u32>,
      pub cursor: Option<String>,           // keyset, matches F1.3
  }
  ```
  This same shape is what [F3.1](3-1-libraries.md) will persist as a
  saved-filter row, so the rule format is shared from the start.
- **Query layer.** New `search_books_advanced(pool, &query)` in
  [db/src/queries.rs](../../db/src/queries.rs). Implementation:
  - Free-text branch reuses `build_fts_match` to generate the FTS5
    `MATCH` expression and joins `books_fts` on `rowid = books.id`.
  - Each facet contributes an `EXISTS (SELECT 1 FROM books_<x>_link
    WHERE book = books.id AND <x> IN (?, ?, ?))` predicate.
  - Keyset pagination on `(books.sort, books.id)` — never `OFFSET` —
    matching the [F1.3](1-3-library-views.md) contract.
  - One JOIN query with `GROUP_CONCAT(authors)` / `GROUP_CONCAT(tags)`
    into the same `BookListItem` DTO [F1.3](1-3-library-views.md)
    already returns.
- **URL state.** The modal serializes `AdvancedSearchQuery` into query
  params on `Apply` (`?q=...&author=12,34&tag=7&publisher=2`) so the
  filtered view is back-button-correct and shareable. The router
  rehydrates the modal selections from the URL on first paint, matching
  what hydrated WASM expects to see in SSR markup.
- **RPC + REST.** Add to
  [frontend/src/rpc.rs](../../frontend/src/rpc.rs):
  `#[get] rpc_search_advanced(query: String) -> Vec<BookListItem>` whose
  body parses the query string and calls `search_books_advanced`. Mirror
  with hand-written `GET /api/books/search` in
  [server/src/backend.rs](../../server/src/backend.rs); both call into
  the same `omnibus_db::queries::search_books_advanced`.
- **Observability.** Emit one `tracing::info!` per advanced-search call
  with `{free_text: bool, facet_count: usize, result_count: usize,
  duration_ms: u64}`. Once [F5.2](5-2-observability.md) lands a metrics
  surface, this becomes a histogram + counter (% of searches that
  include ≥1 facet, p50/p95 latency on advanced-search calls).

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) — shipped. Provides the
  normalized taxonomy tables every facet predicate references.
- [F0.4 FTS5 index](0-4-fts5.md) — shipped. The free-text branch reuses
  `books_fts`.
- [F1.1 Search](1-1-search.md) — shipped. `build_fts_match` is reused
  for the free-text branch.
- [F1.3 Library views](1-3-library-views.md) — must land first or
  concurrently. Shares the keyset cursor + `BookListItem` DTO; this
  initiative should not duplicate either.

## Risks

- **Facet pickers bloat as taxonomies grow.** A library with 5000
  authors can't render a flat dropdown. Mitigated by type-ahead with
  server-side prefix filtering + `LIMIT 50` on every taxonomy listing
  query; selected-but-unfiltered options stay in the chip list
  independently of the dropdown contents.
- **URL length on heavy multi-selects.** Browsers cap at ~2 KB; 200+
  selected ids would exceed that. Accepted — at that selection count
  the user wants a saved-filter library
  ([F3.1](3-1-libraries.md)), not a one-off URL.
- **FTS5 + facet predicate performance.** The combined query plan
  should use `books_fts` first when free-text is present (best
  selectivity), then filter by facet `EXISTS`. Verify with
  `EXPLAIN QUERY PLAN` in the unit tests; if the planner picks a bad
  order on real data, add `INDEXED BY` or rewrite as a CTE.
- **6-month drift:** the structured query shape becomes the
  [F3.1](3-1-libraries.md) saved-filter row format, so a sloppy
  first-pass schema costs a migration later. Mitigated by reviewing the
  `AdvancedSearchQuery` shape with [F3.1](3-1-libraries.md) in mind
  before merge.

## Open questions

**Resolved:**

- **Where does the advanced UI live?** — Popout modal anchored below
  the nav search box (Google-expanded-search style). Decided in the
  planning session; rejects a dedicated `/search` route and inline
  chips.
- **Narrator facet?** — Deferred. The schema has no `narrators` table;
  audiobook metadata extraction is a Phase 2 prerequisite. The
  `AdvancedSearchQuery` struct ships with `#[non_exhaustive]` and a
  commented-out `narrators: Vec<i64>` placeholder so future callers
  can't construct it via positional struct literals; uncommenting the
  field then adds the facet without breaking call sites that use the
  builder/`..Default::default()` path.
- **Mobile parity?** — Yes. Ship `GET /api/books/search` alongside the
  Dioxus server function in this initiative.
- **"Genre" naming?** — Genre maps to existing `tags`. The UI labels
  the picker "Genre / tag" so the distinction never confuses users.

**Unresolved:**

- **AND/OR semantics across facets** — current plan: AND across facets,
  OR within a facet. Is "any author OR any tag" ever a real user need?
  Decision owner: **user**, before TODO 2 (query layer) starts.
- **Saved-filter persistence** — should this initiative ship a "save
  this search" affordance, or is that strictly
  [F3.1](3-1-libraries.md)'s job? Decision owner: **user**, can defer
  until UI work begins.
- **Facet result counts** — show `(123)` next to each option? Cheap
  query (`GROUP BY` on link table) but adds visual noise. Decision
  owner: **user**, during TODO 5 (modal UI).

## TODOs

### 1. Define `AdvancedSearchQuery` + taxonomy DTOs in `shared`

**What:** Add the structured query shape and `TaxonomyOption` DTO to
`shared/src/lib.rs` so server fn, REST handler, and UI all import the
same types.

**Why:** Locks the wire contract before any consumer is written. Keeps
[F3.1](3-1-libraries.md) from inventing a parallel rule shape later.

**Context:** Reserve a commented-out `narrators` slot for the deferred
audiobook story. Derive `Serialize`, `Deserialize`, `Default`,
`PartialEq`.

**Effort:** S
**Priority:** P0
**Depends on:** None.

### 2. `search_books_advanced` query in `omnibus-db`

**What:** New function in `db/src/queries.rs` that takes
`&AdvancedSearchQuery` and returns `Vec<BookListItem>`. Joins
`books_fts` for free-text + `EXISTS (SELECT 1 FROM books_<x>_link …)`
per facet + keyset cursor on `(sort, id)`.

**Why:** The single backend query that powers both web and mobile.

**Context:** Verify plan with `EXPLAIN QUERY PLAN` in tests. Add unit
tests covering: free-text only, single facet only, free-text +
multi-facet, empty query → all books, cursor pagination, non-existent
facet ids → empty results.

**Effort:** M
**Priority:** P0
**Depends on:** TODO 1.

### 3. Taxonomy listing queries

**What:** Add `list_authors_prefix`, `list_series_prefix`,
`list_tags_prefix`, `list_publishers_prefix`, `list_languages_prefix`
to `db/src/queries.rs`. Each takes `(pool, prefix: &str, limit: u32)`
and returns `Vec<TaxonomyOption { id, name, book_count }>`.

**Why:** Powers the type-ahead pickers in the modal.

**Context:** Use `name LIKE ? || '%'` with a `COLLATE NOCASE` index;
the existing taxonomy tables already use `COLLATE NOCASE` on `name`.
`book_count` from `LEFT JOIN books_<x>_link GROUP BY id`.

**Effort:** S
**Priority:** P0
**Depends on:** TODO 1.

### 4. RPC + REST endpoints

**What:** `#[get] rpc_search_advanced(query: String)` in
`frontend/src/rpc.rs` and matching `GET /api/books/search` in
`server/src/backend.rs`. Plus five `rpc_list_<facet>(prefix, limit)`
endpoints for taxonomy.

**Why:** Wire the query layer to web (server fn) and mobile (REST).

**Context:** Both call into `omnibus_db::queries::search_books_advanced`.
REST handler parses query string into `AdvancedSearchQuery`.

**Effort:** M
**Priority:** P0
**Depends on:** TODO 2, TODO 3.

### 5. Advanced-search modal UI

**What:** New `AdvancedSearchModal` component in
`frontend/src/components/top_nav.rs` (or a sibling file if line-count
cap is hit). Multi-select chips per facet with type-ahead dropdowns;
free-text box; Apply / Clear actions. Anchored below the existing
`NavSearch` input. Hidden on `/settings` and `/login`.

**Why:** The discoverable surface this initiative exists to ship.

**Context:** URL serializes selections on Apply; modal rehydrates from
URL on mount. Reuses the `SearchQuery` signal context from
[F1.1](1-1-search.md) plus a new `AdvancedSearchSelection` context.
Match dark `.top-nav` styling.

**Effort:** L
**Priority:** P0
**Depends on:** TODO 4.

### 6. Landing-page rendering wired to advanced query

**What:** Update `frontend/src/pages/landing.rs` so an active
`AdvancedSearchQuery` calls `rpc_search_advanced` instead of (or in
addition to) the current free-text path; results render through the
same `BookListItem` row component as [F1.3](1-3-library-views.md).

**Why:** Closes the loop — modal selections turn into a filtered list.

**Context:** Empty `AdvancedSearchQuery` (no q, no facets) falls
through to the existing full-library list. Single-source-of-truth: do
not branch on `is_advanced` — always call `search_books_advanced` and
let the empty-query branch return everything.

**Effort:** M
**Priority:** P0
**Depends on:** TODO 5.

### 7. Playwright coverage

**What:** New `ui_tests/playwright/tests/flows/advanced-search.spec.ts`
covering: open modal, pick author + tag, Apply, results filter; clear
returns to full list; URL reflects selection; deep-link with query
params auto-opens populated modal; mutation assertion via
`expectMutation` per `.claude/rules/04-playwright.md`.

**Why:** Per `.claude/rules/03-unit-testing.md` +
`.claude/rules/04-playwright.md`, new flows ship with E2E coverage of
the markup contract.

**Context:** Add `data-testid` props on modal opener button, each facet
picker, the Apply button, and the Clear button. Keep names stable.

**Effort:** M
**Priority:** P1
**Depends on:** TODO 6.

### 8. Observability hook

**What:** `tracing::info!` in `search_books_advanced` with
`{free_text, facet_count, result_count, duration_ms}`.

**Why:** Lets us answer "what % of searches use ≥1 facet?" once
[F5.2](5-2-observability.md) metrics land. Cheap to add now, expensive
to retrofit.

**Context:** Use `tracing::Instrument` or `start.elapsed()`. Keep the
log line stable so a future Loki/Tempo query can parse it.

**Effort:** S
**Priority:** P2
**Depends on:** TODO 2.

## Status

In progress — the **search palette** (command-palette / Spotlight pattern) shipped as the first deliverable, replacing the inline nav search input with a floating ⌘K-triggered overlay showing grouped FTS5 results (books, authors, series, tags). This is a lighter, faster surface than the facet-picker modal originally described above. The facet modal's slice-and-dice use case (multi-select facet pickers, structured `AdvancedSearchQuery`, saved filters) moves to [F3.1](3-1-libraries.md).

The palette lives in `frontend/src/components/search_palette.rs` with backing query `db::search_palette` and both RPC (`/api/rpc/search-palette`) and REST (`/api/search/palette`) endpoints. Entity results (author/series/tag) currently navigate to the landing page with a filter; [F1.10](1-10-palette-discovery-link.md) will link them to F1.8 discovery pages once those ship.

---

[← Back to roadmap summary](0-0-summary.md)
