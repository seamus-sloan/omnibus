# F5b — Landing/search keyset pagination (wire-API change)

Status: Proposed — deferred from db.md F5b, awaiting decision.

This doc covers **only** the pagination half of review finding F5. The
mechanical half — collapsing the duplicate `book_files` / `books_series_link`
subquery pairs in `BOOK_COLUMNS` and adding the `(library_id, sort, id)`
index — ships as code in PR **F5a** with no contract change and is not
discussed here except where the two interact. F5b is the part that changes a
wire API and the landing UX, so it needs an operator decision before any code
is written.

---

## Problem

`list_books_for_paths` (`db/src/books/list.rs`, `fetch_list_rows`) materializes
an entire library in one statement —
`SELECT {BOOK_COLUMNS} … ORDER BY b.sort, b.id LIMIT ?` with the bind set to
`MAX_BOOKS_RETURNED = 50_000` (`db/src/books/projection.rs`,
`MAX_BOOKS_RETURNED`). There is no SQL keyset; the cap is the *only* bound. The
module doc on `MAX_BOOKS_RETURNED` says cursor pagination is "intentionally
deferred", so on every landing and search load the whole library is selected,
decoded into `Vec<EbookMetadata>`, run through two more full passes in Rust
(`merge_overrides_into_books`, then `backfill_creator_ids` in `list.rs`),
JSON-encoded, and shipped to the client in one response.

The web path makes this worse: `rpc_get_ebooks` (`frontend/src/rpc.rs`) returns
the full `EbookLibrary` and the landing page sorts and filters it **client-side**
over a hydrated signal — `frontend/src/pages/landing/sorting.rs` re-sorts the
whole `Vec` by any of `SortKey::{Title, Author, Series, LastUpdated,
NewestAdded}` × `SortDir::{Asc, Desc}`, and `filtering.rs` applies the facet
filters in memory. This is a deliberate design (`docs/roadmap/1-3-library-views.md`:
"Client-side sort on already-hydrated lists for libraries ≤10k books … the
biggest perceived-speed win over Calibre-Web"), but it hard-couples "first
paint" to "download the entire library."

Why it bites a named feature:

- **F1.3 Library views** sets the ≤10k-book client-sort ceiling and *also*
  states "Keyset pagination on `(sort, id)` — never `OFFSET`" as the long-term
  fix. The ceiling is a stopgap; above it the current path degrades from "slow
  first paint" to "unusable."
- **F3.1 Libraries with metadata filters** wraps this exact projection in
  additional `WHERE` predicates over normalized columns. Once a saved library
  is "every unread sci-fi book," the query is `filter → temp-sort → cap`, and
  the absence of a `(library_id, sort, id)` index (only `idx_books_sort` on
  `books(sort)` global, and `idx_books_library_id`, exist today —
  `0002_normalized_schema.sql`) forces a full temp-sort. F5a's composite index
  fixes the *sort*; it does not bound the *result-set size*, which is what
  pagination is for.

The truncation signal already exists but is one-directional: the REST handler
`get_ebooks` (`server/src/backend/ebooks.rs`) wraps the response in
`with_pagination_headers` (`server/src/backend.rs`), emitting `X-Total-Count`
and (when truncated) `X-Total-Cap`. The web RPC path can't even do that —
Dioxus server functions don't expose response headers — so the web client has
no way to know it got a truncated library. There is no way to fetch page 2.

---

## Decision required

The operator must decide **how far to take pagination**, which decomposes into
four bound choices:

1. **Cursor key.** The natural keyset is `(b.sort, b.id)` — the existing
   `ORDER BY`. But the web client re-sorts by five different axes after
   hydration. A single `(sort, id)` cursor serves exactly **one** of those ten
   orderings server-side; the other nine are client-side re-sorts over whatever
   page the cursor returned. So the decision is really: **does the cursor own
   the sort order, or does the client?** (You cannot have both with one cursor.)

2. **Cursor encoding.** Opaque base64-of-`(last_sort, last_id)` vs. raw query
   params vs. plain integer `OFFSET`. The roadmap forbids `OFFSET`
   (`1-3-library-views.md`: "never `OFFSET`"; Calibre-Web's offset paging is the
   named large-library killer), which removes one option but not the
   opaque-vs-transparent question.

3. **Surface scope.** Paginate **only the REST/mobile path** (`get_ebooks`,
   which already has headers and a list client), **only the web/RPC path**, or
   **unify** both on one paged `db::list_books_page`. The web path additionally
   requires a UX rearchitecture (infinite-scroll/append) because today it
   assumes a complete Vec.

4. **Page size** and the client's list strategy — fixed page size, and whether
   the web landing moves to infinite-scroll append (roadmap-preferred) or stays
   on the full-Vec model below the 10k ceiling and only paginates above it.

These four are the heart of the doc; the options below are concrete bundles of
answers to them.

---

## Options

All options share the same DB primitive: a `list_books_page(pool, paths,
last_sort: Option<&str>, last_id: Option<i64>, limit: i64)` that appends a
NULL-aware keyset predicate before `ORDER BY b.sort, b.id LIMIT ?`:

```sql
-- after the existing WHERE l.path IN (...)
AND ( :last_sort IS NULL
   OR b.sort > :last_sort
   OR (b.sort = :last_sort AND b.id > :last_id) )
ORDER BY b.sort, b.id
LIMIT :limit
```

`b.sort` is `TEXT COLLATE NOCASE` (`0002_normalized_schema.sql`), so the cursor
comparison and the index must agree on collation — F5a's
`(library_id, sort, id)` index is `COLLATE NOCASE` on `sort` for exactly this.
Rows with `sort IS NULL` collate first under SQLite and need the
`:last_sort IS NULL` short-circuit on the first page; the boundary test below
covers it.

No option requires a table-recreate. SQLite can't drop a column, change a PK,
or alter an FK in place — those force the create-new / copy / drop / rename
dance — but every F5b change is either a pure secondary index (populated on
`CREATE INDEX`) or a Rust SQL-string change. Migrations stay append-only and
forward-only per rule 06.

### Option A — Opaque base64 keyset cursor, REST-first, web stays full-Vec

**How it works.** Add `list_books_page` to the db crate. Extend the REST
`get_ebooks` handler with `?cursor=<base64>&limit=<n>` query params; the cursor
encodes `(last_sort, last_id)`. The server orders by `(sort, id)` only and
returns a `next_cursor` (null at end of stream) alongside the page. The
`X-Total-Count` header stays. The **web/RPC path is unchanged** — `rpc_get_ebooks`
keeps returning the capped full library and the client keeps sorting in memory
below the 10k ceiling.

**Migration shape.** None beyond F5a's `(library_id, sort, id)` index. The
cursor is computed in Rust; no schema. (`base64` 0.22 and `sha2` 0.10 are
already db-crate deps — `db/Cargo.toml` — if we later want to HMAC-sign the
cursor; for an internal app a plain base64 of the tuple is enough since the
cursor is non-authoritative and any tampering only mis-positions the reader's
own page.)

**Blast radius.** db: one new fn + tests. server: `get_ebooks` signature gains
query params, plus a `next_cursor` field — to avoid breaking the
`EbookLibrary` body contract, carry `next_cursor` as a **response header**
(`X-Next-Cursor`) so the JSON body stays byte-compatible for existing mobile
clients, mirroring the `X-Total-Count` precedent. frontend: untouched.

**Pros.** Smallest blast radius. Ships the actual large-library fix where it's
provably needed first (the mobile list client, which has no client-sort luxury).
No web UX rework. Reversible — adding query params with sane defaults is
backward-compatible; the old "no cursor = first page, capped" behavior is the
default branch.

**Cons.** Leaves the web path's "download everything" problem unsolved above
10k. Two divergent read paths (paged REST, full-Vec RPC) until a later increment
unifies them. The cursor only ever serves `(sort, id)`; mobile gets server-sort
order, not the five client axes (acceptable — mobile can sort its own page or
adopt server-side sort keys later).

### Option B — Opaque cursor, unified web + REST, infinite-scroll landing

**How it works.** Option A's db primitive, but **both** surfaces paginate.
`rpc_get_ebooks` gains `last_sort`/`last_id`/`limit` args (RPC POST body, since
`#[get]` server fns can't carry a body — same reason `rpc_search` is POST). The
landing page moves from "hydrate full Vec, sort in memory" to **infinite-scroll
append**. Because the server owns the `(sort, id)` order, the client sort
toolbar must either drive a server-side `sort_key` param (a different `ORDER BY`
+ matching index per axis) or be dropped in favor of server order. This is the
roadmap's stated end-state (`1-3-library-views.md`).

**Migration shape.** Beyond F5a's index, *server-side* sort on the other axes
needs a composite index each — `(library_id, author_sort, id)`,
`(library_id, timestamp, id)`, `(library_id, last_modified, id)`,
`(library_id, series_id, series_index, id)` — one append-only
`CREATE INDEX IF NOT EXISTS` per file. No table-recreate.

**Blast radius.** db: paged fn + a sort-axis → `(ORDER BY, index)` mapping +
per-axis tests. server + frontend/rpc: both signatures change. frontend:
`landing/` is substantially rearchitected — `sorting.rs`/`filtering.rs` move
server-side (or per-page), grid/table gain scroll-sentinel append, and a
`view_prefs` sort change triggers a refetch instead of an in-memory re-sort.
Playwright landing specs change.

**Pros.** One read path. Scales past 10k everywhere. Matches the roadmap
end-state. Kills the whole-library first-paint cost on web.

**Cons.** Largest blast radius by far — a landing-page rewrite, not a db change.
Multi-axis server sort multiplies indexes. **F3.1's dynamic filters interact
badly with multi-axis keyset**: every `(filter, sort)` pair wants its own
covering index, which doesn't scale combinatorially — arbitrary saved-library
predicates fall back to filter-then-temp-sort anyway. Hard to reverse once the
landing UX is rewritten.

### Option C — Hybrid: ship A now, design B's contract but gate the web rewrite

**How it works.** Land Option A (REST cursor + db primitive) immediately.
*Simultaneously* fix the `next_cursor` and `limit` contract shape — header name,
base64 layout, default page size, end-of-stream sentinel — so the eventual web
adoption (Option B) reuses the exact same db fn and wire shape without a second
migration of the contract. Keep the web full-Vec path until a library actually
crosses the 10k ceiling, then flip the landing page to infinite-scroll as a
follow-on PR that touches only `frontend/landing/` and `rpc.rs`, against an
already-proven server primitive.

**Migration shape.** Same as A now (none beyond F5a). B's extra sort-axis
indexes are deferred to the web-adoption PR and only added if multi-axis
*server* sort is actually chosen then (the web rewrite may keep client-sort
*within* a larger page window instead).

**Blast radius.** Now: identical to Option A. Later: identical to Option B's
frontend slice, but de-risked because the db + wire contract is already in
production.

**Pros.** Decouples the irreversible UX decision (B) from the safe, useful db
+ REST win (A). Lets real library sizes drive when — and whether — the web
rewrite is worth it (per assumption A3: most users have ≤100k books, and the
≤10k client-sort ceiling already covers the overwhelming majority). Cheapest
reversibility: A is additive; the B trigger is a product signal, not a
deadline.

**Cons.** Requires getting the cursor/contract shape right up front without the
web consumer in hand — a small risk of designing a header/encoding the web path
later finds awkward. Two read paths coexist for an unbounded interval.

---

## Recommendation

**Option C.** Ship the opaque base64 `(last_sort, last_id)` keyset cursor on the
REST/mobile `get_ebooks` path now, with the `next_cursor`/`limit` contract
designed to be the same one the web path will eventually adopt — but do **not**
rewrite the web landing page in this work.

Rationale:

- **The db primitive is the load-bearing, reusable, low-risk part.**
  `list_books_page` is additive, testable in isolation against
  `sqlite::memory:`, and serves both surfaces unchanged. Building it now retires
  the "no keyset at all" finding regardless of what the web UX becomes.
- **The web rewrite is the irreversible, expensive, product-gated part** and
  the roadmap's own ≤10k client-sort ceiling means it is **not yet on the
  critical path** for the typical install (assumption A3). Spending the landing
  rewrite now would be pre-paying a cost we can defer cheaply.
- **`OFFSET` is off the table** by roadmap mandate; opaque base64 keyset is the
  prescribed shape, and a *signed* cursor is unnecessary for a single-tenant
  self-hosted app — the cursor is non-authoritative (it positions the reader's
  own page; tampering only mis-positions that same reader), so plain base64 of
  the tuple is the right weight. `sha2`/`base64` are already deps if we ever
  want HMAC, so that door stays open at zero new-dependency cost.
- **Single `(sort, id)` cursor is the right scope.** Don't build multi-axis
  server sort (Option B's index fan-out) until the web rewrite forces the
  question — and when it does, F3.1's arbitrary filter predicates mean we'll
  likely keep some client-side or temp-sort path anyway, so over-investing in
  per-axis covering indexes now is speculative.

Cost-of-delay: low. The REST consumer (mobile) is the surface that *can't*
client-sort its way around the problem, so it benefits most and soonest. The web
surface is protected by the 10k ceiling and the existing cap, so deferring its
rewrite costs nothing until a real library crosses that line — at which point
Option C's already-shipped primitive makes the rewrite a frontend-only change.

---

## Migration plan (SKETCH — do not apply)

F5b itself needs **no new schema** beyond F5a's index — the cursor lives in
Rust. The only migration in this neighborhood is F5a's, repeated here for
sequencing clarity:

```sql
-- db/migrations/0019_books_landing_sort_index.sql   (ships in F5a, NOT F5b)
-- Append-only; latest applied is 0018. Pure secondary index, no backfill.
CREATE INDEX IF NOT EXISTS idx_books_library_sort
    ON books (library_id, sort, id);
```

This index is what makes the keyset seek `(b.sort > ? OR (b.sort = ? AND b.id >
?))` an index range-scan instead of a temp-sort under a `library_id` filter.
`sort` is `COLLATE NOCASE`, so the index inherits that collation and the cursor
comparison must run under the same collation (SQLite uses the column's declared
collation for the index automatically).

If Option B/C ever adds server-side multi-axis sort, each axis gets its own
append-only file, e.g.:

```sql
-- db/migrations/00NN_books_sort_axis_indexes.sql   (only if multi-axis server sort lands)
CREATE INDEX IF NOT EXISTS idx_books_library_added    ON books (library_id, timestamp, id);
CREATE INDEX IF NOT EXISTS idx_books_library_modified ON books (library_id, last_modified, id);
```

**Boot backfill:** none. There is no `_norm`-style column to populate. The
cursor is computed at read time from columns that already exist, so there is no
analog to `normalize::backfill_norm_columns` here — nothing in `pool.rs` /
`init_db` changes. (If a future denormalized book-card read-model is built — the
review's stretch suggestion — *that* would need an idempotent indexer-time
backfill modeled on `backfill_norm_columns`; out of scope for F5b.)

---

## Affected code

From the F5 scout spec, scoped to the F5b (pagination) slice:

- `db/src/books/list.rs` — **new** `list_books_page(pool, paths, last_sort,
  last_id, limit)`; `fetch_list_rows` grows the keyset predicate (or a sibling
  `fetch_list_page_rows`). `library_from_db_combined` gains a paged companion.
- `db/src/books/projection.rs` — `MAX_BOOKS_RETURNED` stays as the unpaged
  ceiling / default `limit` source; `BOOK_COLUMNS` and `row_to_ebook` are
  reused unchanged (their collapse is F5a).
- `db/src/books/search.rs` — `fetch_search_rows` already orders by
  `(m.rank, b.sort, b.id)`; a paged search cursor is a **separate, harder**
  problem (the cursor key is the bm25 rank, not `sort`) and is explicitly **out
  of scope** for this doc — list-pagination first.
- `db/src/books/tests.rs` — keyset paging tests (below).
- `server/src/backend/ebooks.rs` — `get_ebooks` gains `?cursor=&limit=` query
  params and emits `X-Next-Cursor`; reuses `with_pagination_headers`
  (`server/src/backend.rs`) for `X-Total-Count`.
- `shared/src/discovery.rs` — `EbookLibrary` body stays byte-compatible
  (cursor rides in a header, like `X-Total-Count`); no field added in Option
  A/C. Only Option B would add a body field.
- `frontend/src/rpc.rs` (`rpc_get_ebooks`) and `frontend/src/pages/landing/*`
  — **untouched in Option A/C**; rewritten only if/when the web-adoption
  follow-on (Option B) is triggered.

---

## Test plan

Per rule 03: sibling `db/src/books/tests.rs`, `sqlite::memory:` via
`test_support::new_in_memory_pool`, long-sentence names, happy + per-variant.

The **acceptance test that must fail on the old schema/code** (it can't even
compile against today's `list_books_for_paths`, which takes no cursor):

- `list_books_page_returns_first_then_next_page_via_cursor_with_no_overlap` —
  seed `page_size + N` books with distinct `sort` values, fetch page 1 with
  `(last_sort=None, last_id=None, limit=page_size)`, derive the cursor from the
  last row, fetch page 2, assert: page 1 has exactly `page_size` rows, page 2
  continues from the boundary, the two pages are disjoint, and their union is
  the full ordered set. This is the test the deferred work is gated on.

Supporting cases:

- `list_books_page_handles_null_sort_rows_at_first_page_boundary` — seed rows
  with `sort IS NULL` (collate first) plus non-null, assert the
  `:last_sort IS NULL` short-circuit returns them on page 1 and the cursor
  advances past them without skipping or duplicating.
- `list_books_page_breaks_sort_ties_by_id_deterministically` — seed multiple
  rows with identical `sort`, assert the `(sort = ? AND id > ?)` arm yields a
  stable, gap-free order across the page boundary (this is what the
  `(library_id, sort, id)` index's trailing `id` exists for).
- `list_books_page_respects_library_path_filter` — two libraries, assert a
  cursor in library A never returns library B rows.
- `list_books_page_returns_empty_at_end_of_stream` — cursor past the last row
  returns zero rows and a null `next_cursor`.

REST layer (`server/src/backend/ebooks.rs` sibling tests, `oneshot` against an
in-memory pool per rule 03):

- `get_ebooks_returns_first_page_and_next_cursor_header_when_truncated`
- `get_ebooks_returns_no_next_cursor_header_at_end_of_stream`
- `get_ebooks_rejects_malformed_cursor_with_400` — a non-decodable base64 /
  bad tuple is a client error, not a 500.

Migration coverage is automatic: `init_db` runs F5a's `0019` against the
in-memory pool, so every db test exercises the new index. `BooksError::Db`
stays covered by the existing pool-closed test; no new error variant is
introduced (a bad cursor is a server-layer 400, decoded before the db call).

Playwright: **no change** under Option A/C — the web landing still loads a full
(capped) library, so existing `landing.spec.ts` contracts hold. Only the Option
B follow-on touches E2E.

---

## Risks & rollback

- **Forward-only, fix-forward.** No down-migrations (rule 06). F5a's index is
  `IF NOT EXISTS` and droppable in a later forward migration if it ever proves
  wrong; the cursor logic is pure Rust and reverts with a code change, no data
  involved.
- **No data-loss surface.** F5b reads only — it adds no column, no destructive
  DDL, no backfill that mutates rows. There is nothing to corrupt and nothing
  irreversible once data accumulates. This is the key reason the db slice is
  safe to ship ahead of the UX decision.
- **Cursor stability across reindex.** `books.id` renumbers on reindex (the
  reason public URLs key on `uuid`, per `rpc_get_ebook`). A cursor is a
  *transient* scroll position, not a durable handle, so a stale cursor after a
  reindex simply mis-positions the next page — acceptable, and identical to how
  any keyset reader behaves under concurrent writes. Do **not** persist cursors.
- **Collation drift.** If the keyset comparison runs under a different
  collation than the index, the seek silently falls back to a temp-sort
  (correct results, lost performance). The tie-break test guards the *order*;
  an `EXPLAIN QUERY PLAN` assertion (or a comment pinning the collation
  contract) guards the *plan*. Note in the F5a index file that the cursor
  depends on its `COLLATE NOCASE`.
- **Irreversible only at the UX layer.** The one genuinely hard-to-reverse step
  is the Option B web rewrite (infinite-scroll replacing the full-Vec model) —
  which is exactly why the recommendation defers it behind a product trigger
  rather than baking it into this change.

---

## Sequencing & dependencies

- **F5a must land first.** The `(library_id, sort, id)` index is what makes the
  keyset seek a range-scan; without it the cursor is correct but slow under a
  library filter. F5b's db primitive depends on F5a's index and on F5a's
  `BOOK_COLUMNS` subquery-collapse being settled (both touch the same SELECT).
- **F3.1 (libraries-as-filters) is the downstream consumer that constrains the
  design.** F3.1 wraps this projection in arbitrary normalized-column
  predicates. A single `(sort, id)` cursor composes cleanly with an added
  `WHERE` (the keyset predicate is just another conjunct); arbitrary
  `(filter, sort)` *server-side* sort does **not** compose without combinatorial
  indexes. This is the strongest argument for the single-cursor scope in the
  recommendation — build the cursor so F3.1 can bolt filters in front of it,
  and let F3.1 decide its own sort story.
- **Search pagination is a separate finding.** `search_books` orders by bm25
  `rank` first; its cursor key is the rank, not `sort`, so it needs its own
  design pass. Do not let F5b's list cursor leak a false assumption that search
  paging is "the same thing."
- **Coupling to the F1↔F2↔F10 user-data chain:** none directly — F5b touches no
  user-data table (progress/sessions/bookmarks/highlights). It is independent of
  that chain and can be sequenced purely against F5a and F3.1.
- **Recommended order:** F5a → F5b (db primitive + REST cursor, Option A/C) →
  [product trigger: a real library crosses ~10k] → Option B web rewrite →
  (separately) search keyset.

---

## Open questions

1. **Default and max `limit`.** What page size? A grid renders ~30–60 cards
   above the fold; the table renders more. Proposal: default 100, hard-cap at
   `MAX_BOOKS_RETURNED` so an unbounded `limit` degrades to today's behavior.
   Operator to confirm.
2. **Cursor opacity vs. debuggability.** Plain base64 of `last_sort\x00last_id`
   is trivially decodable by anyone who looks — fine for a self-hosted app, and
   it keeps cursors greppable in logs. Confirm we don't want HMAC signing (we
   have `sha2` if we change our mind).
3. **Does the web path *ever* adopt server sort, or stay client-sort within a
   bigger page window?** This is the Option B fork and decides whether the
   per-axis index fan-out is ever needed. Defer until the web rewrite is
   actually triggered.
4. **Should `count_books_for_paths` still run on every paged request?** The
   `X-Total-Count` header needs the true total, but recomputing `COUNT(*)` per
   page is wasteful. Option: compute the count only on the first page
   (`last_sort IS NULL`) and let the client cache it. Operator to confirm
   acceptable.
5. **Denormalized book-card read-model** (the review's stretch suggestion) —
   out of scope here, but if list-card hydration cost (the per-row subqueries +
   two follow-up passes) dominates even a *paged* response, a materialized
   read-model refreshed at index time is the next lever. Flag for a separate
   design.
