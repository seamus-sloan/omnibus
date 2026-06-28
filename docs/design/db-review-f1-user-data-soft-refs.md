# F1 — User data: `book_id` CASCADE → `book_uuid` soft-ref

Status: Proposed — deferred from db.md F1, awaiting decision.

This doc frames a data-model decision the operator must make before any
schema or code is written. It is **not** an implementation plan; the SQL
and code below are sketches to make the trade-offs concrete. No `.sql` or
`.rs` file is to be changed until the [Decision required](#decision-required)
section is resolved.

## Problem

Five user-data tables key on the numeric `books.id` with
`ON DELETE CASCADE`:

- `reading_progress`, `bookmarks`, `reading_sessions`, `listening_sessions`
  — `book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE`
  (`db/migrations/0013_reading_progress.sql:15,39,52,65`).
- `highlights` — same shape
  (`db/migrations/0017_highlights.sql:10`).

`books.id` is **not durable**. A reindex's Removed bucket hard-deletes
the row — `DELETE FROM books WHERE library_id = ? AND uuid IN (...)` in
`sync_removed` (`db/src/sync/books.rs:154-160`) — and the cascade silently
takes every reading position, bookmark, session, and highlight with it.
The nuke-and-pave `replace_books` (`db/src/sync/books.rs:538-563`) is
worse: it lists every existing uuid as Removed, so a single full reindex
of a library wipes **all** user data for that library. `books.id` also
moves when `library_id` cascades from a settings library-path change, so
even a path edit can destroy the lot.

The roadmap forbids exactly this pattern. `docs/roadmap/3-2-ratings-journaling.md:30`:
"Do not use `book_id` (INT FK with `ON DELETE CASCADE`) for user data.
Cascade-delete is appropriate for ephemeral derived data (covers, FTS
index) but not for user-generated data." It prescribes a `book_uuid TEXT
NOT NULL` soft reference (no FK, no cascade) so a pruned book's row
*detaches* and auto-relinks when the same uuid reappears.

The schema is already internally inconsistent: `metadata_overrides`
follows the correct pattern (`book_uuid TEXT NOT NULL PRIMARY KEY`, no
FK — `db/migrations/0007_metadata_overrides.sql:16-22`), and `sync_books`
explicitly documents leaving it untouched across reindex
(`db/src/sync/books.rs:77-80`). The five user-data tables are the
holdouts.

Who this bites: **F3.2 ratings/journaling** and **F3.4 stats** (which
reads `reading_sessions`/`listening_sessions`) build directly on these
tables. Once real ratings, journals, and session history accumulate, a
reindex or path change is a data-loss event with no recovery — and the
fix gets more expensive the longer it waits, because the migration has to
preserve live user rows rather than empty schema.

Two adjacent defects fall inside the same column and should travel with
this change:

- **F9** — `record_session_tx` resolves the book with a bare
  `SELECT id FROM books WHERE uuid = ?` (`db/src/progress.rs:150`) that
  ignores `merged_uuids`, so a session for a since-merged book is silently
  dropped (`Ok(false)`). The canonical resolver `resolve_book_id_by_uuid`
  (`db/src/books/get.rs:132-145`) already folds in `merged_uuids`; the
  write paths `upsert_progress` / `create_highlight` use it
  (`db/src/progress.rs:63`, `db/src/highlights/mod.rs:36`).
- **Merge omits highlights** — `move_progress_and_history`
  (`db/src/merge/transaction.rs:298-340`) re-parents `reading_progress`
  (with latest-wins dedupe), `bookmarks`, `reading_sessions`, and
  `listening_sessions` by `book_id`, but never touches `highlights`. A
  manual merge today loses the source book's highlights. The rewrite to
  `book_uuid` must add `highlights` to the re-parent set.

## Decision required

The operator must resolve two coupled model choices. Everything else in
this doc is mechanical once these are fixed.

**Decision 1 — Survival semantics.** When a book is removed from disk and
its `books` row is gone, soft-ref user rows now survive as orphans. Pick:

- **(1a) Survive indefinitely, no GC.** Orphan rows linger forever and
  auto-relink if the same uuid reappears on a future scan. Matches
  `metadata_overrides` behavior exactly. Simplest; unbounded orphan growth
  (overlaps F10's no-GC concern).
- *From owner: Ideally, books can exist in Omnibus without a file existing
  in the future. Books should be able to be added to a "wishlist." This
  feature isn't added to the roadmpa yet, but should be accounted for none-
  theless.*

**Decision 2 — `BookNotFound` write guard.** Today `upsert_progress` and
`create_highlight` reject writes for an unknown uuid
(`ProgressError::BookNotFound` / `HighlightError::BookNotFound`). With a
soft ref we could:

- **(2a) Keep the guard.** A write still resolves the uuid against
  `books`/`merged_uuids` first; you cannot record progress for a book the
  server has never indexed. Smallest blast radius — the resolver call
  stays, only the *stored* column changes from `id` to `uuid`.
- *From owner: There shouldn't be a time where the user has progress on
  on a book that doesn't exist.*

These are co-dependent with **F2** (the uuid must be *stable* — a
path-derived `stable_uuid` re-keys every book on a library-root change, so
even uuid-soft-ref rows orphan) and **F10** (a shared reconcile/GC that
`metadata_overrides` also needs). See
[Sequencing & dependencies](#sequencing--dependencies).

## Options

All three options share the **same migration** (Option A below is the
migration; B and C layer survival policy on top). SQLite reality
constrains the shape: you cannot drop an FK/cascade or change a column
type in place, so each table needs a full **table-recreate dance**
(`CREATE … _new`, `INSERT … SELECT` backfill, `DROP`, `RENAME`, recreate
indexes/CHECK/UNIQUE). Migrations are append-only and forward-only
(rule 06).

### Option A — Soft-ref columns only, survive-no-GC (Decision 1a + 2a)

**How it works.** Recreate the five tables with `book_uuid TEXT NOT NULL`
replacing `book_id`, dropping the FK and cascade. Backfill `book_uuid`
in-migration by joining the old `book_id` to `books.uuid`. Write paths
keep the resolver (guard stays) but store and key on `book_uuid` instead
of resolving to `id`. No boot GC: orphans linger and auto-relink, exactly
like `metadata_overrides`.

**Migration shape.** One `NNNN_user_data_soft_ref.sql` (number allocated at
implementation time), five table
recreates. Backfill is the `INSERT … SELECT … JOIN books` itself — no
boot-time pass needed, because every existing row is already
uuid-addressable through its current `book_id`. Rows whose book is already
gone (already lost today) are simply not carried forward.

**Blast radius.** `db/src/progress.rs`, `db/src/highlights/mod.rs`,
`db/src/merge/transaction.rs` and their sibling tests; server integration
tests. No new boot code path. ~L effort, concentrated in the migration +
the three modules.

**Pros.** Matches the one correct precedent in the schema
(`metadata_overrides`); smallest new surface; no destructive boot path to
get wrong; fully reversible-in-spirit (orphans are harmless data, not
deletions). **Cons.** Unbounded orphan growth over time; dead rows slow
any future `LEFT JOIN` against these tables (the same drag F10 notes for
`metadata_overrides`); defers the GC question rather than answering it.

## Recommendation

**Adopt Option A: soft-ref columns only, survive-no-GC, keep the write
guard (Decision 1a + 2a).** Defer GC to a combined **F1 + F10** cleanup
once F2 (stable uuid) has landed.

Rationale:

- **A is the only option that doesn't add a new data-loss surface.** The
  entire point of F1 is to stop reindex from silently destroying user
  data. Option B's boot-time `DELETE` re-introduces that risk if the grace
  window is wrong; shipping it in the same change as the protection itself
  is the wrong order of operations.
- **It matches the one correct precedent.** `metadata_overrides` survives
  without GC today and the system tolerates it. Treat the five user-data
  tables identically; revisit orphan accumulation for *all* soft-ref
  tables at once in F10, not piecemeal.
- **Orphan growth is not yet a real problem.** These tables are
  schema-only-to-lightly-used this cycle; the join-drag F10 worries about
  is negligible at current volumes. GC is a real feature with its own
  grace-window design — it deserves its own change, not a rider.
- **The write guard is cheap insurance.** Keeping `resolve_book_id_by_uuid`
  at the write boundary costs one query and blocks garbage uuids from ever
  entering the tables. Relaxing it (Option C) buys a Phase-4 capability no
  current client uses.

**Reversibility / cost of delay.** The migration is forward-only and, once
it runs against any DB, frozen (rule 06). But A is the *low-regret* base:
it changes only the reference column and preserves all live rows, so a
later F1+F10 GC layers cleanly on top without re-touching the schema. The
cost of *delaying the whole change* is the opposite — every rating,
journal entry, and session that accumulates before the migration is a row
the migration must carry, and every reindex in the meantime is a live
data-loss event. Land A before F3.2 populates these tables.

## Migration plan

> **Sketch only — not to be applied until the decision is ratified.**

`db/migrations/NNNN_user_data_soft_ref.sql` — the next zero-padded number is
allocated at implementation time (`0018_multiformat_book_files.sql` is the
latest applied as of writing). SQLite cannot drop a cascade FK or
change a column in place, so each of the five tables is a full recreate.
`PRAGMA foreign_keys` is per-connection (ON at runtime); DDL inside the
migration runs fine. The backfill is the `INSERT … SELECT … JOIN books`
itself — no separate boot pass.

Pattern, shown for `reading_progress` (repeat for all five):

```sql
CREATE TABLE reading_progress_new (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id                INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  book_uuid              TEXT    NOT NULL,            -- soft ref: no FK, no cascade
  format                 TEXT    NOT NULL CHECK (format IN ('epub','audio')),
  epub_cfi               TEXT,
  audio_position_seconds REAL,
  updated_at             INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  CHECK ((format='epub'  AND epub_cfi IS NOT NULL AND audio_position_seconds IS NULL)
      OR (format='audio' AND audio_position_seconds IS NOT NULL AND epub_cfi IS NULL)),
  UNIQUE (user_id, book_uuid, format)
);
INSERT INTO reading_progress_new
  (id,user_id,book_uuid,format,epub_cfi,audio_position_seconds,updated_at)
  SELECT rp.id, rp.user_id, b.uuid, rp.format, rp.epub_cfi,
         rp.audio_position_seconds, rp.updated_at
  FROM reading_progress rp JOIN books b ON b.id = rp.book_id;
DROP TABLE reading_progress;
ALTER TABLE reading_progress_new RENAME TO reading_progress;
CREATE INDEX reading_progress_user_book_idx ON reading_progress(user_id, book_uuid);
```

Per-table notes for the remaining four:

- **`bookmarks`** — no UNIQUE today; keep that. Index
  `bookmarks_user_book_idx(user_id, book_uuid)`.
- **`reading_sessions` / `listening_sessions`** — **preserve the
  `device_id INTEGER REFERENCES devices(id) ON DELETE SET NULL` FK**; that
  cascade is on *device*, not book, and is correct. Recreate
  `*_sessions_user_book_idx(user_id, book_uuid)`. (Note: the windowed
  `(user_id, started_at)` index F-session-stats wants is a *separate*
  finding; do not fold it in here unless explicitly scoped.)
- **`highlights`** — preserve the `color` CHECK + `'amber'` default, the
  `note` column, and `created_at`. Index
  `idx_highlights_user_book(user_id, book_uuid)`.

**Boot backfill:** **none** for Option A — the `INSERT … SELECT` JOIN does
all the work in-migration. (Option B *would* add an idempotent
`reconcile_user_data(pool)` to `db/src/normalize.rs`, called from
`db/src/pool.rs:56` after `backfill_norm_columns`, shaped like
`backfill_norm_columns` — no-op once caught up. Not part of the
recommended scope.)

## Affected code

From the scout spec, scoped to Option A:

- **`db/migrations/NNNN_user_data_soft_ref.sql`** *(new)* — table-recreate
  all five tables to `book_uuid TEXT NOT NULL` soft-ref; backfill via
  `INSERT … SELECT JOIN books`; recreate every index/CHECK/UNIQUE;
  preserve the `device_id` FK on the two session tables.
- **`db/src/progress.rs`** — `upsert_progress`, `get_progress`,
  `record_session_tx`, `record_session`. Bind/store `book_uuid` directly;
  key the `ON CONFLICT` and the read-back on `(user_id, book_uuid,
  format)`. Keep the `resolve_book_id_by_uuid` call for the guard but use
  it to confirm existence + resolve the **canonical uuid** (not the id).
  Fix `record_session_tx`'s bare `SELECT id FROM books WHERE uuid = ?` to
  resolve through the same merged-uuid-aware path — **closes F9**.
- **`db/src/highlights/mod.rs`** — `create_highlight` stores `book_uuid`;
  `list_highlights` and `get_highlight_by_id` drop the
  `JOIN books b ON b.id = h.book_id` and read `book_uuid` as a direct
  column; `row_to_highlight` reads `book_uuid` instead of `uuid` from the
  join.
- **`db/src/merge/transaction.rs`** — rewrite `move_progress_and_history`
  to dedupe + re-parent on `book_uuid` (`source_uuid` → `target_uuid`),
  and **add `highlights` to the re-parent set** (currently missing).
  `merge_books` already holds both uuids, so pass them through.
- **`.claude/architecture.md`** — note the soft-ref convention now covers
  the five user-data tables (rule 99 docs sync).

## Test plan

Per rule 03: sibling `<mod>/tests.rs`, `sqlite::memory:`, happy path +
one test per `thiserror` variant. The **acceptance test that must fail on
the old schema** is the reindex-survival test.

- **`db/src/progress/tests.rs`** — retarget existing raw assertions from
  `WHERE book_id = ?` to `WHERE book_uuid = ?`; `seed` returns the uuid.
  Keep the happy-path upsert + `BookNotFound` variant tests. **Add** the
  acceptance regression: seed a book + a `reading_progress` row, run
  `replace_books` with an empty set (full Removed), assert the row
  **survives** (`book_uuid` still present). This must fail on the old
  cascade schema. **Add** an F9 test: a session whose uuid is a
  `merged_uuid` resolves to the surviving canonical uuid and persists
  (`record_session` returns `Ok(true)`), not `Ok(false)`.
- **`db/src/highlights/tests.rs`** — same retarget; **add** the
  reindex-survival test for `highlights`.
- **`db/src/merge/tests.rs`** — assert `move_progress_and_history`
  re-parents all five tables **including `highlights`** by uuid, and that
  the `reading_progress` latest-wins dedupe still holds across the uuid
  key.
- **`server/src/backend/progress/tests.rs`** and
  **`server/src/backend/highlights/tests.rs`** — drive
  `rest_router(AppState::new(pool))` via `tower::ServiceExt::oneshot`
  against an in-memory DB: POST a progress/highlight, trigger a reindex,
  GET it back, assert `200` + data intact.

Run: `cargo test -p omnibus-db`, then `cargo test -p omnibus`, then
`just test` for the full matrix.

## Risks & rollback

- **Forward-only, fix-forward.** The new migration is frozen once it runs
  anywhere (rule 06); any correction is a later migration. There are no
  down-migrations.
- **Backfill drops already-orphaned rows.** The `INSERT … SELECT JOIN
  books` does not carry rows whose `book_id` no longer joins to a `books`
  row. Those rows are *already lost* under the current cascade, so this is
  a no-op in practice — but it is worth stating that the migration is not a
  recovery mechanism for data the old schema already destroyed.
- **What's irreversible once data accumulates:** the choice of `book_uuid`
  as the durable key is load-bearing across `metadata_overrides`,
  `merged_uuids`, cover filenames, and route URLs. This is precisely why
  **F2 (durable stored uuid) must land first or alongside** — migrating to a
  soft ref *before* F2 buys a smaller, not zero, data-loss surface, because
  until F2 the path-derived uuid still moves on a repoint. Once F2 lands the
  uuid is stored and never recomputed, so the soft ref is fully durable. See
  sequencing.
- **Option B-specific (not recommended now):** a boot-time GC `DELETE`
  with a too-aggressive grace window would delete legitimately-detached
  rows — the exact failure F1 exists to prevent. Keeping GC out of this
  change removes that risk entirely.

## Sequencing & dependencies

The F1 ↔ F2 ↔ F10 chain:

- **F2 (durable stored uuid + relative-path scan key) must land first, or
  in the same train.** *Before* F2, the path-derived uuid is recomputed every
  scan and a repoint cascade-deletes books, so a soft ref on that uuid still
  orphans every row on a library-root change — the row survives as an orphan
  but cannot auto-relink, because the uuid itself moved. F2 fixes this at the
  root: `books.uuid` becomes a stored value that is never recomputed, and a
  repoint reuses the scan-root row (and no longer prunes books) instead of
  deleting it — so once F2 lands the soft ref is fully durable. F1 without F2
  is a partial fix; the roadmap lists the uuid fix as a hard dependency of
  F3.2 (`docs/roadmap/3-2-ratings-journaling.md:46-57`). Recommended order:
  **F2 → F1 (Option A) → F3.2**.
- **F9 (`record_session_tx` merged-uuid resolution) folds into F1.** It
  touches the same `record_session_tx` column and resolver; fix it in the
  same change (covered in [Affected code](#affected-code) and
  [Test plan](#test-plan)).
- **The latent merge-highlights omission folds into F1.** Same module,
  same rewrite.
- **F10 (shared reconcile/GC for soft-ref tables) comes after.** Once F1
  lands, `metadata_overrides` and the five user-data tables all have the
  same orphan-without-GC shape. F10 designs one reusable, grace-windowed
  reconcile for all of them — that is the right home for Decision 1b, not
  this change.
- The scout flags conflicts with **F11** and **F12** (other db.md
  findings); confirm those don't co-edit the same migration ordering before
  scheduling, but they are not blockers for the decision framed here.

## Open questions

1. **Grace window shape (resolved by F10).** F10 has now answered this:
   soft-detach via a `detached_at` column (uuid absent from both `books.uuid`
   and `merged_uuids` → mark detached, filter from reads, hard-purge after a
   30-day retention measured from detach time). The user-data tables here
   **inherit the soft-detach disposition** — they must never hard-delete,
   since a journal entry is not regenerable the way a metadata override is.
   See [db-review-f10-override-gc.md](db-review-f10-override-gc.md).
2. **Migration ordering with F2 (resolved).** F2 does **not** re-key
   `books.uuid` — it keeps existing values verbatim and only adds a `scan_key`
   column — so this migration's `INSERT … SELECT JOIN books` backfill produces
   the same `book_uuid` whether or not F2 has run. The only coupling left is
   migration *numbering*: F1 and F2 must take distinct, ordered `NNNN` numbers
   (F2 lower if stacked). No data-ordering hazard.
3. **Override-cover orphans (F10 territory).** `metadata_overrides`
   orphans also leak `override-<uuid>.<ext>` cover files. The five
   user-data tables have no file side-effects, so this is not an F1
   concern, but the eventual GC should sweep both — noted so F10 scopes it.
4. **Should `bookmarks` gain a UNIQUE on the soft-ref migration?** Today it
   has none. The recreate is a natural moment to add one if the model
   wants at-most-one-bookmark-per-position, but that is a behavior change
   out of scope for F1 — default is to preserve current (no UNIQUE).
