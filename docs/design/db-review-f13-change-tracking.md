# F13 — Phase-4 device-sync change tracking

Status: Proposed — deferred from db.md F13, awaiting decision.

This doc frames the data-model decision that must land *before* any
schema or code is written. It does not prescribe an implementation; it
gives the operator the choice, the trade-offs, and a recommended path
with an honest accounting of cost-of-delay and reversibility.

---

## Problem

Phase-4 device sync (Kobo F4.1, OPDS-incremental) needs the server to
answer one question per device: *"what changed since your last sync?"* —
including **deletions**, so a device can drop a book that is no longer in
the library. The current schema cannot answer that question.

Three concrete gaps, confirmed against the code:

1. **No monotonic cursor.** `books.last_modified` is `TEXT NOT NULL
   DEFAULT (datetime('now'))` at one-second resolution
   (`db/migrations/0002_normalized_schema.sql:34`, indexed at `:80`). It
   is overwritten **in place** on every Changed book — see the
   `last_modified = datetime('now')` clause in `update_book_row`
   (`db/src/sync/books.rs:484`) and `update_audiobook_row`
   (`db/src/sync/audiobooks.rs`). A second-resolution, in-place,
   non-gap-free high-water mark is unusable as a sync cursor: two books
   changed in the same second collapse to one timestamp, and a client
   that syncs mid-second can miss a row forever.

2. **Deletions vanish.** Removed books are hard-`DELETE`d through the
   `ON DELETE CASCADE` FK on `books` (`0002:26`). `sync_removed`
   (`db/src/sync/books.rs:125`) clears the FTS twin then deletes the
   `books` row; the audiobook Removed branch does the same inline
   (`db/src/sync/audiobooks.rs:50-76`). Once the row is gone there is no
   record it ever existed, so the sync endpoint **cannot** tell a device
   "this book is gone." The device keeps a phantom book indefinitely.

3. **No per-device cursor store.** F4.1 names a `kobo_sync_tokens` table
   keyed on user (`docs/roadmap/4-1-kobo-sync.md:30`) but lists its only
   schema dependency as the uuid index — already satisfied by
   `uuid TEXT NOT NULL UNIQUE` (`0002:25`). That undersells the gap: the
   token table is the *easy* part; the change feed it points into does
   not exist.

Why it bites a named feature: F4.1's `/kobo/v1/library/sync` is, by
spec, "only books changed since the client's last token," and Kobo's
protocol requires explicit delete notifications. The roadmap promotes
native Kobo sync ahead of OPDS precisely because it is the better UX on
the platform that matters most (`4-1-kobo-sync.md:13`). Delivering it on
today's schema means a retrofit onto the hot indexer write path under
deadline — exactly the refactor this doc exists to pre-empt.

This is a **data-model** gap, not a missing endpoint. The endpoint is
cheap once the feed exists; the feed is what needs deciding now.

---

## Decision required

The operator must pick **one** primary model and resolve two secondary
questions. All three are recorded here so a later implementer inherits a
settled design, not an open one.

1. **Primary — change-tracking model.** Either
   - **(A) Soft-delete + monotonic `change_seq` on `books`** — every
     read path must learn to filter `deleted_at IS NULL`; or
   - **(B) A separate `book_changes` audit feed** written by the sync
     writers — read paths and hard-delete semantics untouched.

   This is the heart of the decision: A and B diverge in **which files
   they touch**, and A is expensive to walk back once read paths depend
   on `deleted_at`.

2. **Secondary — counter mechanism.** SQLite has no sequences. The
   monotonic source is either a single-row `change_counter` table bumped
   inside the sync transaction (works for A *and* B), or
   `INTEGER PRIMARY KEY AUTOINCREMENT` on the audit table (B only). Pick
   per the model chosen.

3. **Secondary — cursor scope.** Is the change feed **global** (one
   sequence space for the whole library) or **per-user**? The library
   today is single-tenant at the catalog level — every user sees the
   same books — so a global feed with per-`(user, device)` cursors is
   the natural fit. Per-user change rows only become necessary if/when
   per-user catalog visibility (shelves, private uploads) lands, which is
   not in any current phase.

---

## Options

All DDL below is **additive** (`ADD COLUMN` + `CREATE TABLE/INDEX`). No
option alters a primary key, drops a column, or changes an FK, so **none
require the SQLite table-recreate dance** (`CREATE new` → `INSERT … SELECT`
→ `DROP old` → `ALTER … RENAME`). Migrations stay append-only and
forward-only per rule 06.

### Option A — soft-delete `deleted_at` + monotonic `change_seq` on `books`

**How it works.** Add two columns to `books`: `change_seq INTEGER` (the
cursor) and `deleted_at INTEGER` (epoch secs; NULL = live). A single-row
`change_counter` table is bumped inside the sync transaction; every
insert/update stamps the new value into `books.change_seq`, every removal
sets `deleted_at` and bumps `change_seq` **instead of** deleting the row.
A device syncs by selecting `books WHERE change_seq > :cursor ORDER BY
change_seq`, mapping `deleted_at IS NOT NULL` to a delete notification.

**Migration shape.**
```sql
ALTER TABLE books ADD COLUMN change_seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE books ADD COLUMN deleted_at INTEGER;            -- NULL = live
CREATE INDEX idx_books_change_seq ON books(change_seq);
CREATE INDEX idx_books_deleted_at ON books(deleted_at);
CREATE TABLE change_counter (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  value INTEGER NOT NULL DEFAULT 0
);
INSERT INTO change_counter (id, value) VALUES (1, 0);
```

**Blast radius — large.** Soft-delete is viral: **every** read path that
lists or counts books must add `deleted_at IS NULL`, or soft-deleted
books leak back into the UI. That means `BOOK_COLUMNS` and the
list/get/search queries in `db/src/books/projection.rs`, plus browse,
discovery (`get_author`/`get_series`), taxonomy CTEs, and the FTS-backed
search join. The cascade also changes meaning: today removing a book
cascade-clears `book_files`, link rows, and the FTS twin
(`0002:26`, `sync_removed`). Under soft-delete the operator must decide
whether those satellite rows stay (so a re-add restores the book whole)
or still get cleared (so only the `books` tombstone survives) — and the
FTS twin **must** be cleared regardless, or deleted books keep matching
search.

**Pros.** One sequence space, gap-free, trivially correct cursor
arithmetic. No second table to garbage-collect. "Current state" and
"change record" are the same row — no join to resolve what a book looks
like now.

**Cons.** Highest blast radius and the **hardest to reverse**: once a
dozen read paths filter `deleted_at`, backing out means re-auditing all
of them. A forgotten read path is a latent correctness bug (deleted book
reappears) that won't surface until Phase-4 actually soft-deletes
something. `books` accumulates tombstone rows forever unless a separate
reaper is built (which then has to respect the *slowest* device cursor).

### Option B — separate `book_changes` audit feed

**How it works.** Leave `books` and its hard-delete cascade exactly as
they are. Add an append-only `book_changes` table; the sync writers emit
one row per mutation (`'upsert'` on insert/update, `'delete'` in
`sync_removed` **before** the cascade fires). The row is keyed by
`book_uuid` (a *soft* reference, no FK) so it survives the hard-delete —
the same durable-reference pattern `metadata_overrides` uses
(`0007_metadata_overrides.sql`, and rule 06's book-identity guidance). A
device syncs by selecting `book_changes WHERE seq > :cursor ORDER BY
seq`, joining live `books` for upserts and reporting `kind='delete'`
rows as removals.

**Migration shape.**
```sql
CREATE TABLE book_changes (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,   -- monotonic cursor
  book_uuid  TEXT    NOT NULL,                     -- soft ref; survives delete
  kind       TEXT    NOT NULL,                     -- 'upsert' | 'delete'
  changed_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE INDEX idx_book_changes_uuid ON book_changes(book_uuid);
```
`AUTOINCREMENT` (not bare `INTEGER PRIMARY KEY`) is deliberate: it
guarantees a strictly increasing `seq` that is **never reused** even
after the highest row is deleted by the reaper — a reused seq would let a
caught-up device silently skip a change.

**Blast radius — small and contained.** Read paths are untouched —
`projection.rs`, browse, discovery, taxonomy, search all stay exactly as
they are. The only write-path change is one `record_change(tx, uuid,
kind)` call added at four existing choke points (`insert_book_row`,
`update_book_row`, `sync_removed`, and the three audiobook mirrors). No
new `deleted_at IS NULL` predicate anywhere.

**Pros.** Minimal, localized blast radius; near-zero correctness risk to
existing features (a forgotten call site under-reports a change to a
device, never corrupts the library). Hard-delete semantics preserved, so
the cascade keeps satellite tables clean for free. Trivial to GC: delete
`book_changes` rows below the slowest device cursor, with coalescing
(only the latest row per uuid matters to a fresh device). Easy to walk
back — drop the table, remove four calls.

**Cons.** A second table to write and eventually GC. Resolving "what does
this book look like now" requires a join back to `books` (for `upsert`
rows). A book changed N times before a device's first sync produces N
rows unless coalesced — handled by the GC's per-uuid coalescing, not a
hot-path concern.

### Option C — do nothing now; lean on `last_modified`

**How it works.** Defer entirely; when F4.1 lands, widen
`last_modified` to a monotonic integer and bolt on a tombstone table
under deadline.

**Why it's listed and rejected.** It is the implicit status quo, so it
belongs in the comparison. But it pushes the *exact same* data-model
decision into the middle of F4.1 implementation, where it competes with
protocol work for attention and lands as a rushed retrofit on the hot
write path. Cost-of-delay is real but the *cost-of-deciding* is what this
doc removes; C throws that away. Not recommended.

---

## Recommendation

**Adopt Option B (separate `book_changes` audit feed), with a global
sequence space (`AUTOINCREMENT`) and per-`(user_id, device_id)`
cursors.** Land the new migration now (additive, zero read-path risk) and
the writer wiring; defer the GC/reaper and all `/kobo/*` endpoints to
F4.1.

Rationale:

- **Reversibility dominates here.** This is forward-looking infra for a
  feature that isn't built yet; the operator should keep the cheapest
  exit. B is a table plus four call sites — trivially droppable. A bakes
  `deleted_at` into a dozen read paths; once data accumulates and the UI
  depends on the filter, unwinding it is a second migration *plus* a
  read-path re-audit. Pay the reversible cost, not the irreversible one.

- **Blast radius matches confidence.** We do not yet know F4.1's exact
  needs (Kobo's protocol is undocumented; the roadmap scopes it as
  "parity with Calibre-Web," `4-1-kobo-sync.md:42`). Touching only the
  write path keeps the uncommitted surface small until the protocol is
  pinned down.

- **It composes with the existing soft-reference grain.** `book_uuid`
  with no FK is the same durable-reference pattern as
  `metadata_overrides` and the `merged_uuids` resolver
  (`resolve_book_id_by_uuid` in `db/src/books/get.rs:132` already
  UNION-falls-back through `merged_uuids`). The change feed slots into a
  pattern the codebase already endorses, so a delete row keyed by uuid is
  idiomatic, not novel.

- **Cost-of-delay is bounded but not zero.** The longer we wait, the more
  the eventual backfill must synthesize. B's backfill is a one-time
  "emit one `upsert` per existing book" seed — bounded by library size,
  idempotent, identical in spirit to `backfill_norm_columns`. A's
  backfill (`change_seq = rowid` for every book) is comparable, but A's
  *ongoing* cost — every new read path remembering the filter — never
  ends. B has no recurring tax.

The one place A genuinely wins — no join to resolve current state — does
not justify viral read-path changes for infra we may tune again when F4.1
forces the protocol's real shape into view.

**Both options ship `kobo_sync_tokens` unchanged** (it is orthogonal to
A-vs-B):
```sql
CREATE TABLE kobo_sync_tokens (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id  TEXT    NOT NULL,
  last_seq   INTEGER NOT NULL DEFAULT 0,   -- cursor into book_changes.seq
  updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  PRIMARY KEY (user_id, device_id)
);
```

---

## Migration plan (SKETCH — do not apply yet)

`db/migrations/NNNN_change_tracking.sql` — the number is allocated at
implementation time (next after the highest on disk, currently
`0018_multiformat_book_files.sql`). Forward-only, additive only,
so **no table-recreate dance** is needed:

```sql
-- NNNN_change_tracking.sql  (Option B, recommended)
CREATE TABLE book_changes (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  book_uuid  TEXT    NOT NULL,
  kind       TEXT    NOT NULL,            -- 'upsert' | 'delete'
  changed_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE INDEX idx_book_changes_uuid ON book_changes(book_uuid);

CREATE TABLE kobo_sync_tokens (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id  TEXT    NOT NULL,
  last_seq   INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  PRIMARY KEY (user_id, device_id)
);
```

**Boot backfill** — add a `backfill_initial_changes(pool)` step in
`init_db` (`db/src/pool.rs`), called right after
`backfill_norm_columns(&pool)` at `pool.rs:56`, mirroring its idempotency
contract exactly:

- Guard: `SELECT 1 FROM book_changes LIMIT 1` — if any change row exists,
  return early (no-op once caught up). Same shape as
  `backfill_norm_columns`'s `if rows.is_empty() { return Ok(()) }` guard
  (`db/src/normalize.rs:53`).
- Otherwise, in one transaction, `INSERT INTO book_changes (book_uuid,
  kind) SELECT uuid, 'upsert' FROM books` so a device's very first sync
  sees the whole library.

This makes a pre-migration database converge with a fresh one on the next
boot, with no manual step — the rule-06 backfill contract.

(If Option A is chosen instead, the migration adds the two `books`
columns + indexes + `change_counter` single-row table per the Option A
sketch above, and the backfill stamps `change_seq = id` for every
existing book under the same idempotency guard.)

---

## Affected code

From the scout spec, scoped to **Option B** (A's extra read-path files
are noted inline):

- `db/migrations/NNNN_change_tracking.sql` — **new**: `book_changes` +
  `kobo_sync_tokens` + indexes (Option A: `books.change_seq` /
  `books.deleted_at` columns + `change_counter` instead).
- `db/src/change_feed/{mod.rs,tests.rs}` — **new module**:
  `record_change(tx, uuid, kind)` writer and `changes_since(pool,
  cursor)` reader; `kobo_sync_tokens` get/advance accessors. Module
  `//!` + per-`pub` `///` per rule 05; `thiserror` enum with
  `#[error(transparent)] #[from] sqlx::Error` per rule 02 — never leak
  raw `sqlx::Error` across the boundary (the `SyncError` pattern at
  `db/src/sync/books.rs:25` is the local model).
- `db/src/sync/books.rs` — `insert_book_row` (emit `upsert`),
  `update_book_row` (`:484`, emit `upsert`), `sync_removed` (`:125`,
  emit `delete` **before** the cascade DELETE at `:154-160`). Extract a
  shared `record_book_change(tx, …)` helper rather than inlining, to stay
  under the rule-05 ~80-line cap.
- `db/src/sync/audiobooks.rs` — mirror in `insert_audiobook_row`,
  `update_audiobook_row`, and the inline Removed branch (`:50-76`).
- `db/src/sync.rs` — update the module `//!` doc; re-export the new
  `change_feed` writer if it lives under `sync/`.
- `db/src/pool.rs` — `init_db`: call `backfill_initial_changes` after
  `backfill_norm_columns` (`:56`).
- **Option A only:** `db/src/books/projection.rs` (`BOOK_COLUMNS` +
  list/get/search), `db/src/browse.rs`, `db/src/discovery/*`,
  `db/src/taxonomy.rs`, and the FTS search join all gain
  `deleted_at IS NULL`. This file set is the blast-radius difference
  between the two options.
- `docs/roadmap/4-1-kobo-sync.md` — update the schema-dependency note
  (`:29-30`) to point at this initiative; record `.claude/architecture.md`
  per rule 99.

---

## Test plan

Per rule 03: sibling `db/src/change_feed/tests.rs`, `sqlite::memory:` via
`test_support`, happy path + one test per `thiserror` variant. The
migrator runs on every `init_db("sqlite::memory:")`, so the new migration is
exercised transitively by the whole suite.

New `db/src/change_feed/tests.rs`:

- `record_change_emits_upsert_when_book_inserted` — a new book produces
  one `upsert` row.
- `record_change_seq_is_monotonic_when_book_updated_twice` — two updates
  produce strictly increasing `seq` values.
- **The acceptance test that must fail on the old schema:**
  `removed_book_is_still_reportable_after_books_row_is_gone` — remove a
  book, assert a `delete` change row exists keyed by the (now-deleted)
  uuid. On today's hard-delete schema there is no feed at all, so this
  test cannot even compile against `main` without the new migration — it is the
  red bar proving the gap is closed. (Option A variant: assert
  `deleted_at IS NOT NULL` and `change_seq` bumped, and that the row is
  excluded from `list_books`.)
- `changes_since_returns_only_rows_above_cursor_in_seq_order` — and is
  empty when caught up.
- `kobo_sync_tokens_cursor_round_trips_per_user_and_device` — get/advance
  is scoped to `(user_id, device_id)`.
- `changes_since_propagates_db_error_when_pool_is_closed` — the
  `Db`/`#[from] sqlx::Error` variant.

Extend `db/src/sync/tests.rs`:

- `sync_books_writes_change_rows_inside_committed_transaction` — assert a
  rolled-back tx leaves **no** change row (atomicity with the data
  write).

`db/src/pool.rs` (or `normalize`-adjacent) backfill test:

- `backfill_initial_changes_seeds_one_upsert_per_pre_migration_book` and is a
  no-op on second call (idempotency, mirroring the
  `backfill_norm_columns` test).

Option A only: add read-path tests proving soft-deleted books are
excluded from list/browse/discovery/search/taxonomy.

---

## Risks & rollback

- **Forward-only, fix-forward.** Migrations are append-only (rule 06).
  An error in the new migration is corrected by a later one, never by
  editing it once applied anywhere — `sqlx` checksums each file and a changed
  applied migration fails startup.
- **Option B is cheaply reversible** *until devices hold cursors.* Drop
  `book_changes` + `kobo_sync_tokens` and remove four call sites. Once
  real Kobo devices have synced and stored a `last_seq`, dropping the
  feed strands those cursors — but that only happens in F4.1, long after
  this lands.
- **Option A is the irreversible surface.** Once read paths depend on
  `deleted_at IS NULL` and tombstone rows accumulate, you cannot drop the
  column without a table-recreate migration *and* re-auditing every read
  path. A single missed read path is a silent correctness bug
  (soft-deleted book reappears) that won't surface until something is
  actually soft-deleted in Phase-4. This asymmetry is the core reason the
  recommendation is B.
- **Data-loss surface.** Both options are additive — neither destroys
  existing data on apply. The live risk is the **GC/reaper** (deferred to
  F4.1): if it prunes `book_changes` rows below the *slowest* device
  cursor incorrectly, a lagging device misses changes. The reaper must
  read `MIN(last_seq)` across `kobo_sync_tokens` before deleting, and
  coalesce rather than gap the sequence. Out of scope for this migration;
  flagged so it isn't forgotten.
- **Counter contention.** Both the single-row `change_counter` (A) and
  `AUTOINCREMENT` (B) serialize on the sync transaction. Sync is already
  a single transactional choke point (`sync_books` at `:81`), so this
  adds no new contention — the write was already serialized.

---

## Sequencing & dependencies

The scout flags F13 as coupling with **F1, F11, F12, F14, F17, F18**.
The load-bearing ordering constraints:

- **F1 ↔ F2 ↔ F10 (book-identity chain) must settle first.** F13's
  change rows are keyed by `book_uuid` as a durable soft reference. If a
  later finding changes how book identity is anchored (path-derived
  `stable_uuid` is itself flagged in db.md as fragile, and `merged_uuids`
  already complicates uuid resolution), the change-feed key inherits that
  decision. Land the identity model before wiring the feed key, or the
  feed points at an unstable anchor. The `resolve_book_id_by_uuid`
  fallback (`db/src/books/get.rs:132`) is the existing seam the reader
  must respect.
- **F14 / F17 / F18 (other schema-pass findings).** If any of these
  also add `books` columns or touch the sync write path, batch them into
  the same adjacent schema pass so the hot write path is
  instrumented once, not thrice. Coordinate the migration numbering so
  they don't collide.
- **F4.1 depends on F13, not the reverse.** This doc *unblocks* F4.1 —
  `kobo_sync_tokens` and the change feed are F4.1's listed prerequisites
  (`4-1-kobo-sync.md:30,35`). F13 should land in the next schema pass so
  F4.1 consumes a ready feed rather than building one under deadline.
- **What must land first:** the book-identity decision (F1 chain). What
  can land in parallel: nothing blocks the *table* creation, but the
  *writer wiring* should wait on identity so the key is final.

---

## Open questions

1. **Cursor scope confirmation.** This doc assumes a **global** sequence
   space with per-`(user, device)` cursors, justified by today's
   single-tenant catalog. If per-user catalog visibility (private
   uploads, per-user shelves) is on any horizon, the feed may need a
   `user_id` dimension — decide before authoring the migration since adding
   it later is a non-additive change to `book_changes`.
2. **Reading-state vs. catalog changes.** F4.1 routes *reading state*
   (bookmarks, progress) through F2.1 internally (`4-1-kobo-sync.md:31`),
   separate from *catalog* changes. This doc covers only catalog change
   tracking (book add/update/delete). Confirm the two feeds stay
   separate — conflating them would over-couple F13 to F2.1.
3. **Coalescing policy for the backfill + GC.** Does a device's first
   sync need one row per book (simple) or can the reader coalesce on the
   fly? Affects whether the backfill emits per-book rows or the reader
   collapses them. Recommend per-book backfill + GC-time coalescing;
   confirm at F4.1 implementation.
4. **kepubify cache invalidation hook.** F4.1 invalidates the KEPUB cache
   on `book.last_modified` bump (`4-1-kobo-sync.md:21`). If `change_seq`
   (Option A) or a `book_changes` upsert (Option B) becomes the canonical
   "this book changed" signal, the invalidation should key off that, not
   the coarse `last_modified`. Note for F4.1, not a blocker for this migration.
