# Series model: `series_index` placement vs. many-to-many `books_series_link`

Status: Proposed — deferred from db.md F14, awaiting decision.

This doc frames a single data-model choice — **may a book belong to more than
one series?** — and lays out the two coherent schema resolutions so the
operator can decide before the series browse/detail pages cement the current
arbitrary behaviour. It does not propose application code beyond the migration
sketch; nothing here is to be applied until the decision lands.

---

## Problem

`books_series_link` is structurally a many-to-many join table —
`PRIMARY KEY(book, series)` with **no `UNIQUE(book)`**
([0002_normalized_schema.sql:61](../../db/migrations/0002_normalized_schema.sql)) —
so the schema permits a book to link to N series. But a book's
position-within-series is a **single scalar on the work row**,
`books.series_index REAL`
([0002_normalized_schema.sql:31](../../db/migrations/0002_normalized_schema.sql)),
not on the membership row. The data model therefore claims "a book can be in
many series" while only being able to store one position for it. A book that is
#1 in one series and #4 in another cannot represent both numbers.

The read path has already silently resolved this contradiction by **picking one
series arbitrarily**. The shared projection's series subqueries
(`BOOK_COLUMNS`,
[db/src/books/projection.rs:55-63](../../db/src/books/projection.rs)) select
`series_name` and `series_link_id` with
`... FROM books_series_link bsl JOIN series s ... WHERE bsl.book = b.id ORDER BY s.name LIMIT 1`.
If a book ever held two series rows, both the surfaced name and the surfaced id
would be whichever series sorts first alphabetically, and the other membership
would vanish from every list, detail, and search response.

**The bug is latent, not active.** The application is single-series end-to-end
today, so the join table never holds more than one row per book:

- The wire type is single-valued: `EbookMetadata.series: Option<String>` /
  `series_index: Option<String>`
  ([shared/src/ebook/metadata.rs:61-62](../../shared/src/ebook/metadata.rs)).
- The write path inserts **at most one** link row per book —
  `insert_metadata_links` resolves `m.series` (a single `Option<String>`) and
  runs one `INSERT OR IGNORE INTO books_series_link`
  ([db/src/sync/books.rs:665-672](../../db/src/sync/books.rs)). Its own comment
  states the assumption outright: *"Series / publisher / language are
  single-valued per book, so they keep the simple resolve-then-link path"*
  (books.rs:656-657).
- The override store mirrors the scalar shape — `metadata_overrides` carries
  `series` / `series_index` as single JSON scalars edited from one frontend
  form.
- Both override-aware read CTEs read the index off the work row
  (`b.series_index`): `EFFECTIVE_SERIES_CTE` in
  [db/src/discovery/series.rs:61-95](../../db/src/discovery/series.rs) and
  `series_index_sql` in [db/src/browse.rs](../../db/src/browse.rs) (~138-202).

So `LIMIT 1` always returns the single existing row and `series_index` is
unambiguous **only as long as the one-series-per-book convention holds** — a
convention enforced nowhere in the schema.

**Why it bites a named roadmap feature.** F3.1 (Shelves,
[docs/roadmap/3-1-shelves.md](../roadmap/3-1-shelves.md)) builds smart-shelf
rules over the normalized columns, and series-by-detail / browse-
by-series pages are the surfaces that will read `series_name` and order by
`series_index`. Once those pages and any series filter rule
(`series = ?`, `ORDER BY series_index`) ship, they harden around whichever model
the schema implicitly has. Choosing afterward means re-deriving an answer under
pressure with live data and live UI contracts in the way. The cost of deciding
is lowest **now**, while the join table provably holds ≤1 row per book.

---

## Decision required

**Product question (the heart of this doc): is a book allowed to belong to more
than one series at once, each with its own position?**

Concrete examples that force the call:

- A novella that is both #2 in *The Expanse* main sequence and #1 in a curated
  *Expanse: Novellas* collection.
- An omnibus edition that is #1 in a publisher's "definitive editions" series
  while its constituent works sit at #3–#5 of the original series.
- A short story anthologized into two different themed collections.

If the answer is **no** (one canonical series per book — the model every current
code path already assumes), the schema should *say so* and stop the read path
from silently arbitrating. If the answer is **yes**, the position number has to
move onto the membership row and a per-series position must ripple through the
wire type, parser, write path, both read CTEs, and the frontend.

A non-answer is itself a choice — it keeps a many-to-many table with an
arbitrary-pick read, the worst of both worlds. The decision must be made before
F3.1's series surfaces land.

---

## Options

SQLite reality that constrains all options: you **cannot** add a `UNIQUE`
constraint, change a `PRIMARY KEY`, drop a column, or alter an FK in place. Each
requires the table-recreate dance (`CREATE … _new` / `INSERT … SELECT` /
`DROP` / `ALTER … RENAME`). Migrations are append-only and forward-only per
[rule 06](../../.claude/rules/06-migrations.md); the same migrator runs against
`sqlite::memory:` in tests, so a migration green in the suite is green in
production.

### Option A — Commit to single-series; add `UNIQUE(book)`

**How it works.** Declare the product model explicitly: one series per book.
Add `UNIQUE(book)` to `books_series_link` so the schema enforces the 1:1 that
the parser, wire type, write path, and override store already assume.
`series_index` stays on the `books` row — now provably unambiguous because each
book has ≤1 series. The `ORDER BY s.name LIMIT 1` in the projection becomes
honest (it can only ever return one row), and `BOOK_COLUMNS` gains a one-line
invariant comment instead of a behaviour change.

**Migration shape.** Table-recreate of `books_series_link` adding
`UNIQUE(book)`, with a deterministic dedup collapse
(`SELECT book, MIN(series) GROUP BY book`) so the new constraint can't fail on
any pre-existing duplicate. In practice this collapse affects **0 rows** on
current data, but it makes the migration safe on any DB that somehow accumulated
a second row. `series_index` is untouched. Sketch in
[Migration plan](#migration-plan).

**Blast radius.** Schema-only. No change to the parser, `EbookMetadata`, the
write path (the existing single `INSERT OR IGNORE` is already
`UNIQUE(book)`-compatible), or either read CTE. Two new tests; one optional
invariant comment in `projection.rs`. Effort **S–M**.

**Pros.**
- Matches reality everywhere; closes the latent arbitrary-pick by construction.
- Cheapest path, smallest diff, lowest review surface.
- Makes a real invariant machine-checked instead of convention-only.

**Cons.**
- Forecloses multi-series. Reversing later (→ Option B) is a second
  table-recreate plus the full B ripple — not free, but no worse than doing B
  from scratch.
- A future Calibre import where one book legitimately appears in two collections
  silently keeps only `MIN(series)` (collapse) or fails the second
  `INSERT OR IGNORE` (which already no-ops today). Acceptable only if "no" is
  genuinely the product answer.

### Option B — Commit to multi-series; move `series_index` onto the link row

**How it works.** Move the position number off the work and onto the membership:
`books_series_link(book, series, series_index)`, keeping `PRIMARY KEY(book, series)`
(no `UNIQUE(book)`). Each membership carries its own position. Read paths surface
**per-series** position; the projection stops `LIMIT 1`-picking and instead
returns a list of `{series_id, series_name, series_index}` memberships. The wire
type grows a `Vec<SeriesMembership>` (or similar), the parser learns to emit more
than one membership, and both override-aware CTEs are rewritten to read
`bsl.series_index` from the link row rather than `b.series_index`.

**Migration shape.** Table-recreate of `books_series_link` adding
`series_index REAL`, backfilled from the current `books.series_index` for the
single existing membership per book. `books.series_index` is left in place
(dead) or dropped via a second forward-only recreate of `books`. Recreate
`idx_books_series_series`; add `idx_books_series_link_index` for ordering.

**Blast radius.** Large and cross-cutting — this is the ripple the scout spec
flags as XL:
- `shared/src/ebook/metadata.rs` — new multi-membership wire field; every
  consumer of `EbookMetadata.series` / `series_index` / `series_id`.
- `db/src/ebook/parse.rs` — `collect_series` must return N memberships, not one.
- `db/src/sync/books.rs` — write path moves the index onto the link insert and
  loops over memberships.
- `db/src/books/projection.rs` — `BOOK_COLUMNS` series subqueries become a
  `json_group_array` aggregate; `row_to_ebook` decodes a list.
- `db/src/discovery/series.rs` (`EFFECTIVE_SERIES_CTE`) and
  `db/src/browse.rs` (`series_index_sql`) — both rewritten to read the link-row
  index, including the override-merge logic that currently coalesces
  `b.series_index`.
- Frontend series UI — book detail must render multiple series + positions;
  the override edit form must edit per-membership index.

Effort **XL**. Gated on its own implementation plan — this doc only frames it.

**Pros.**
- The only model that can represent the legitimate two-series cases above.
- No data is ever silently discarded by an arbitrary pick.

**Cons.**
- Largest change in the repo's data layer; touches every series read and the
  wire contract.
- All cost is paid up front for a feature with no current product demand
  (no UI, no parser source, no user request on the roadmap).
- Higher ongoing complexity in every series query forever.

### Option C — Defer (status quo): do nothing

Keep the many-to-many table with the scalar index and the arbitrary-pick read.
**Not recommended.** It leaves the contradiction in place and lets F3.1's series
surfaces harden around undefined behaviour. Listed only to name it as the
default-if-nothing-decided, which the operator should explicitly reject.

---

## Recommendation

**Take Option A: commit to single-series and add `UNIQUE(book)`.**

Rationale:

1. **It matches every existing decision in the codebase.** The parser, the wire
   type, the write path (with its explicit "single-valued per book" comment),
   the override store, and both read CTEs were all written single-series. Option
   A makes the schema agree with code that already shipped; Option B contradicts
   it everywhere.
2. **No product demand for multi-series exists.** The roadmap names no feature
   requiring a book in two series; F3.1 filters on `series = ?`, which a
   single-series model serves cleanly. Paying Option B's XL cost now is
   speculative.
3. **It closes the actual finding.** The defect is that the read path arbitrates
   silently. Option A removes the arbitration by making >1 membership impossible;
   Option B removes it by surfacing all memberships. Both fix F14; A is an
   order of magnitude cheaper.

**Reversibility and cost-of-delay.** Option A is not a one-way door, but it is a
*sticky* one. While `books_series_link` provably holds ≤1 row per book, the
collapse is a no-op and reversing to B costs exactly what doing B from scratch
costs. The irreversibility risk grows only **after** a multi-series import path
exists and real second-series rows accumulate — at which point A's
`MIN(series)` collapse would silently drop user data. That is the trigger to
revisit, not today. The cost of *delaying the decision* is higher than the cost
of picking A: every series surface shipped under the status quo is one more
consumer hardened around arbitrary-pick that a later B (or A) must rewrite.

Pick A now; gate any future B on this doc plus a concrete multi-series feature.

---

## Migration plan

> **Sketch only — not to be applied.** DDL below is illustrative and lands only
> after the decision is recorded. The migration number (`NNNN`) is allocated at
> authoring time — the next free number then (highest on disk is currently
> `0018`). Forward-only; never edit an applied migration
> ([rule 06](../../.claude/rules/06-migrations.md)).

### Option A — `db/migrations/NNNN_series_link_unique.sql`

```sql
-- F14: commit to single-series. Add UNIQUE(book) to books_series_link so the
-- 1:1 the parser / wire type / write path already assume is enforced by the
-- schema, and the projection's ORDER BY s.name LIMIT 1 is provably a no-op.
-- SQLite can't add a UNIQUE constraint in place -> table-recreate.
PRAGMA foreign_keys=OFF;

CREATE TABLE books_series_link_new (
    book   INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    series INTEGER NOT NULL REFERENCES series(id),
    PRIMARY KEY(book, series),
    UNIQUE(book)                         -- enforces one series per book
);

-- Deterministic collapse: keep the lowest series id per book so the UNIQUE
-- insert cannot fail on any pre-existing duplicate. 0 rows affected on
-- current data; defensive for any DB that accreted a second row.
INSERT INTO books_series_link_new (book, series)
  SELECT book, MIN(series) FROM books_series_link GROUP BY book;

DROP TABLE books_series_link;
ALTER TABLE books_series_link_new RENAME TO books_series_link;

-- Recreate the reverse index dropped with the old table.
CREATE INDEX idx_books_series_series ON books_series_link(series);

PRAGMA foreign_keys=ON;
-- series_index stays on books — now unambiguous (each book has <=1 series).
```

Notes:
- The `PRAGMA foreign_keys=OFF; … ON;` bracket keeps the `DROP` of the old table
  from tripping its own `series` FK during the swap. Each migration file runs in
  the migrator's implicit transaction against `sqlite::memory:` in tests.
- **No boot backfill is required.** Unlike `backfill_norm_columns` in
  `normalize.rs` (which fills `title_norm` / `author_norm` at boot because SQL
  can't compute them), Option A computes everything in DDL — the collapse INSERT
  is the entire data step and is idempotent by construction. No `pool.rs` /
  `normalize.rs` change.
- No write-path change: `insert_metadata_links`
  ([db/src/sync/books.rs:665-672](../../db/src/sync/books.rs)) already inserts a
  single row via `INSERT OR IGNORE`, which satisfies `UNIQUE(book)`. Optionally
  add a `// invariant: UNIQUE(book) => <=1 row` comment near `BOOK_COLUMNS`
  ([projection.rs:55-63](../../db/src/books/projection.rs)) documenting why
  `LIMIT 1` is now provably safe.

### Option B — `db/migrations/NNNN_series_index_on_link.sql` (sketch, not recommended)

```sql
-- F14 (multi-series): move position onto the membership row.
PRAGMA foreign_keys=OFF;
CREATE TABLE books_series_link_new (
    book         INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    series       INTEGER NOT NULL REFERENCES series(id),
    series_index REAL,                   -- moved off books
    PRIMARY KEY(book, series)
);
INSERT INTO books_series_link_new (book, series, series_index)
  SELECT bsl.book, bsl.series, b.series_index
    FROM books_series_link bsl JOIN books b ON b.id = bsl.book;
DROP TABLE books_series_link;
ALTER TABLE books_series_link_new RENAME TO books_series_link;
CREATE INDEX idx_books_series_series       ON books_series_link(series);
CREATE INDEX idx_books_series_link_index   ON books_series_link(series_index);
PRAGMA foreign_keys=ON;
-- books.series_index left dead (or dropped via a second forward-only recreate
-- of books). Option B's app-code ripple is XL and gated on its own plan.
```

After either migration, run `just dev-bounce` so a live `dx serve` picks up the
new file — migrations apply only at startup (rule 06).

---

## Affected code

From the scout spec, by option.

**Option A (recommended):**

| File | Symbol | Change |
|---|---|---|
| `db/migrations/NNNN_series_link_unique.sql` | new file | table-recreate adding `UNIQUE(book)` + collapse INSERT |
| `db/src/books/projection.rs` | `BOOK_COLUMNS` (55-63) | optional invariant comment; **no behaviour change** |
| `db/src/sync/books.rs` | `insert_metadata_links` (665-672) | **none** — existing `INSERT OR IGNORE` already compatible |
| `db/src/sync/tests.rs` | new tests | UNIQUE-rejection + reindex-idempotence |

**Option B (not recommended; XL — listed for completeness):**

| File | Symbol |
|---|---|
| `db/migrations/NNNN_series_index_on_link.sql` | new file — add `series_index` to link table + recreate indexes |
| `shared/src/ebook/metadata.rs` | `EbookMetadata.series` / `series_index` / `series_id` (61-66) → multi-membership field |
| `db/src/ebook/parse.rs` | `collect_series` (≈159-161) → N memberships |
| `db/src/sync/books.rs` | `insert_metadata_links` (665-672); `update_book_row` / `insert_book_row` `series_index` binds |
| `db/src/books/projection.rs` | `BOOK_COLUMNS` series subqueries; `row_to_ebook` `series_index` read (148/182) |
| `db/src/discovery/series.rs` | `EFFECTIVE_SERIES_CTE` (61-95), `fetch_series_books`, `count_effective_series_members` |
| `db/src/browse.rs` | `list_series` / `series_index_sql` (≈121-202) |
| `db/src/test_support.rs` | seed factory writing `series_index` (≈252, 420) → write index on link row |

---

## Test plan

Per [rule 03](../../.claude/rules/03-unit-testing.md): sibling `<mod>/tests.rs`,
`sqlite::memory:` via `init_db("sqlite::memory:")` (which runs every migration,
so the new migration is smoke-tested by the whole suite), happy path + one test
per failure variant. Names in the long-sentence `fn_under_test_does_X_when_Y` style.

For **Option A**, in `db/src/sync/tests.rs` (the sibling file already exists):

1. **The acceptance test that must fail on the old schema** —
   `books_series_link_rejects_second_series_for_same_book`: seed one book + one
   series via the `test_support` factories, then run a **raw**
   `INSERT INTO books_series_link (book, series)` for a *second, distinct*
   series id and assert it returns a `UNIQUE`-constraint `sqlx::Error`. Raw
   INSERT (not the write path, which uses `INSERT OR IGNORE` and would swallow
   the conflict) is what proves the constraint exists. **On the pre-migration
   schema this insert succeeds** — that's the regression guard.
2. `series_link_unique_is_idempotent_for_reindex`: run the book replace/sync
   path twice over the same fixture and assert exactly **one**
   `books_series_link` row per book — guards that
   `wipe_per_book_link_rows` + re-insert still satisfies `UNIQUE(book)`.
3. Keep the existing `series_index_sorts_numerically`
   (`db/src/sync/tests.rs:659`) green — it asserts `REAL` ordering on the books
   row and is unchanged by Option A.

Run `cargo test -p omnibus-db`.

For **Option B**, the test surface is far larger: parser emits N memberships,
projection returns a membership list, both CTEs order by the link-row index, and
the wire round-trips a `Vec`. Out of scope here — it ships with B's own plan.

---

## Risks & rollback

- **Forward-only, fix-forward.** There are no down-migrations. A bad migration is
  corrected by a *new* one, never by editing the applied file (rule 06's
  checksum mismatch fails startup otherwise).
- **Option A data-loss surface (currently empty).** The collapse INSERT keeps
  `MIN(series)` per book. On any DB where a book genuinely has two series rows it
  would **discard the higher-id membership**. Current data has zero such rows
  (no code path inserts a second), so the collapse is a no-op today — but this is
  the one place A can lose data, and it is exactly why the decision must precede
  any future multi-series import.
- **What becomes irreversible.** Once a multi-series *import path* exists and
  real second-series rows accumulate, switching A→B can no longer recover the
  pre-collapse memberships (they were dropped at migration time). Before that
  point, A↔B is reversible at the cost of one more table-recreate. The trigger to
  revisit is "a feature wants a book in two series," not calendar time.
- **Option B risk** is breadth, not data loss: a rewrite of every series read
  and the wire contract, each a chance to regress series ordering or the override
  merge. Mitigated only by the large test surface above — another reason to
  prefer A unless multi-series is a committed product goal.

---

## Sequencing & dependencies

The scout spec marks F14 as conflicting with **F1, F6, F8, F10, F16, F17, F18** —
all of which touch the same hot tables/files (`books`, `books_series_link`,
`BOOK_COLUMNS`, the sync write path). Practical ordering:

- **F1 ↔ F2 ↔ F10 chain (stable_uuid / user-data soft-refs / overrides GC).**
  These are the schema's correctness core and reshape book identity and the
  reindex reconcile step. F14 does **not** depend on their outcome — it touches
  only the `series` membership shape — but landing F14 Option A as an isolated,
  schema-only migration *before* the larger identity rework keeps it off the
  critical path and avoids merge churn in the recreate DDL. Sequence F14-A early
  and small; do not entangle it with the identity chain.
- **F6 / F8 (landing projection / browse rewrites).** These rewrite the queries
  that *read* series (`BOOK_COLUMNS`, `series_index_sql`). Option A leaves those
  reads untouched, so order is free. Option B **must** land before or with F6/F8
  or it re-rewrites them — another reason A de-risks the sequence.
- **F16/F17/F18 (timestamps, redundant/speculative indexes, dead columns).**
  Independent cleanup migrations; no ordering constraint with F14. Note F14-A
  recreates `idx_books_series_series` — coordinate the migration number so it
  doesn't collide with an index-cleanup migration authored in parallel.

**Must land first:** nothing. F14-A is self-contained. **Should land before
F14:** nothing — but F3.1's series surfaces and any series filter rule **must
land after** the decision, or they harden around the arbitrary pick this doc
exists to remove.

---

## Open questions

1. **Product call (blocking):** does any roadmap-tracked feature require a book
   in two series? If a stakeholder can name one, Option B's cost is justified and
   this doc's recommendation flips. If not, Option A stands.
2. **Calibre import fidelity:** does the source library format Omnibus imports
   from ever express multi-series membership we'd want to preserve? If Calibre
   itself is single-series (it is, via `series` + `series_index` columns), that
   is strong corroboration for Option A.
3. **`books.series_index` index under Option B:** if B is ever chosen,
   `idx_books_series_index` on the now-dead `books.series_index` should be dropped
   in the same migration that moves the index — fold that into B's plan, not a
   separate cleanup.
4. **Invariant documentation:** under Option A, should the `UNIQUE(book)`
   assumption be asserted in code (e.g. a debug-only check that the projection
   never sees >1 series row) in addition to the schema constraint, or is the
   schema constraint + the new test sufficient? Recommendation: schema + test is
   enough; skip the runtime check.
