# Unify machine timestamps on INTEGER unix-seconds (F11)

**Status: Proposed — deferred from db.md F11, awaiting decision.**

Source: `docs/review/db.md` → *"Three inconsistent time representations"*. This
doc exists to make the one blocking decision tractable — the **wire/serde shape
for `books.timestamp` / `books.last_modified`** — and to scope the migration that
follows. No schema or Rust changes are made here; the DDL below is a sketch.

---

## Problem

The schema encodes "machine" timestamps three different ways:

1. **INTEGER unix-seconds** (the target) via `strftime('%s','now')` — all auth
   tables (`db/migrations/0004_auth.sql:26-71`), the progress/session tables
   (`0013_reading_progress.sql:19,42,53-55,67-68`), `highlights.created_at`
   (`0017_highlights.sql:15`), `libraries.last_indexed` (`0002:19`),
   `book_files.mtime_epoch` (`0009:15`).
2. **TEXT** via `datetime('now')` (`'YYYY-MM-DD HH:MM:SS'`) — `books.timestamp`
   and `books.last_modified` (`0002_normalized_schema.sql:33-34`),
   `metadata_overrides.updated_at` (`0007:21`), `author_photos.fetched_at`
   (`0008:18`), `merge_log.merged_at` / `merge_log.undone_at` (`0016:53-54`).
3. **DATETIME** via `CURRENT_TIMESTAMP` — `ignored_authors.ignored_at`
   (`0010:18`). SQLite has no real `DATETIME` affinity, so this is the same
   fixed-width TEXT as (2), just a third spelling.

`books.pubdate` (`0002:32`) is a genuinely partial **human** date and is
correctly excluded — it stays TEXT.

**Why it bites today (not just cosmetics):**

- `books.timestamp` / `books.last_modified` are read raw as `String` into
  `EbookMetadata.added_at` / `modified`
  (`db/src/books/projection.rs:33,176,188`;
  `shared/src/ebook/metadata.rs:53,83-88`), and the landing page sorts those
  **lexicographically** as strings (`frontend/src/pages/landing/sorting.rs:71-78`
  → `RowKey.plain: Option<String>` compared via `String: Ord`). This works
  *only because* fixed-width zero-padded ISO sorts the same as chronological
  order. The instant a writer stores a bare unix-second string instead, the
  sort silently corrupts (`'9999999999' > '10000000000'`).
- `db/src/covers.rs` `get_last_modified_epoch` already pays for the mismatch:
  `SELECT CAST(strftime('%s', last_modified) AS INTEGER)` — a per-call string
  parse that would be a plain column read if the column were INTEGER.

**Why it bites a named roadmap feature:** F3.4 stats
(`docs/roadmap/3-4-stats.md:18`) aggregates the INTEGER session tables with
`GROUP BY date(start_at, 'unixepoch')` and `SUM(end_at - start_at)`. Any join or
unified timeline that mixes `books.timestamp` ("date added") or a future
`journal.created_at` with those session tables can't `GROUP BY date(...)`
uniformly: INTEGER columns need `date(col,'unixepoch')`, TEXT ones need bare
`date(col)`. F3.2 ratings/journaling specs *more* TEXT `updated_at` columns
adjacent to the INTEGER session tables, so the divergence compounds rather than
stabilizes. Mixed types also defeat a single Rust-side serialization helper.

The convention to standardize on is INTEGER unix-seconds — the auth migration
argued it and every table since has followed it. This finding migrates the five
laggard tables to match.

---

## Decision required

Two decisions, one of which is load-bearing:

**D1 (blocking) — the wire/serde shape for `added_at` / `modified`.** The DB
columns *will* become INTEGER regardless. The open question is whether that type
change is allowed to cross the HTTP/RPC boundary:

- **Keep the wire `String`** (`EbookMetadata.added_at/modified: Option<String>`
  stay as-is), formatting the epoch back to fixed-width ISO on the DB read path.
  Frontend untouched, lexicographic sort still correct. **OR**
- **Change the wire to `i64`** (`Option<i64>`), and fix the frontend sort to be
  numeric and the table cell to format the epoch for display.

**D2 (mechanical, not really optional) — migration mechanism.** SQLite cannot
`ALTER COLUMN` a type, cannot redefine a `NOT NULL DEFAULT (datetime('now'))` in
place, and cannot drop a column on old engines. So every affected table is
**recreated** (`CREATE … _new` → `INSERT…SELECT` with `strftime('%s', col)` →
`DROP` → `RENAME`), converting existing values in the same statement. This is the
only viable shape; it's listed as a "decision" only so the operator signs off on
the table-recreate of `books` (heavily FK-referenced) and the consequent index
re-creation.

The rest of this doc assumes D2 = table-recreate and focuses on resolving D1.

---

## Options

All three migrate the DB columns to INTEGER identically; they differ only on D1
(the wire shape) and therefore on blast radius.

### Option A — DB columns INTEGER, wire stays `String` (DB-side ISO formatting)

**How it works.** In `db/src/books/projection.rs` `BOOK_COLUMNS`, wrap the two
columns:
`strftime('%Y-%m-%dT%H:%M:%SZ', last_modified, 'unixepoch') AS modified` (and
the same for `timestamp AS added_at`). `build_metadata` keeps
`r.get::<String,_>(...)`. `shared::EbookMetadata` and the entire `frontend/`
sort + render path are **untouched**.

**Migration shape.** `0019_*.sql` recreates the five tables → INTEGER.

**Blast radius.** DB crate only. `shared/` and `frontend/` see zero diff. No
change to `sorting.rs` / `table.rs` / `filtering.rs` or their fixtures.

**Pros.** Smallest surface; isolates the change to the crate that owns the
columns; preserves the existing lexicographic-sort correctness invariant by
construction (ISO output is still fixed-width). Lowest conflict footprint against
the other in-flight PRs that touch `projection.rs` (see Sequencing).

**Cons.** The wire contract stays "stringly-typed time," so a *future* consumer
still can't do arithmetic on `added_at` without re-parsing. Adds a tiny
`strftime` cost to every book read (negligible vs. the existing `CAST` already in
`covers.rs`, and the projection already runs subqueries per row).

### Option B — DB columns INTEGER, wire becomes `i64`

**How it works.** `EbookMetadata.added_at/modified: Option<i64>`
(`shared/src/ebook/metadata.rs:53,83-88`). `projection.rs` reads the INTEGER
columns directly. `frontend/src/pages/landing/sorting.rs` `RowKey.plain` can no
longer be a single `Option<String>` that serves title/author/time keys — the
time keys need numeric compare. Either retype `plain` to carry an enum
(string-or-i64) or add a parallel `epoch: Option<i64>` field and branch in
`cmp_with_missing_last`. `table.rs:305-306` must format the epoch to a human
string for display; `filtering.rs` fixtures (`140-163,181-202`) and
`sorting.rs` fixtures (`281-302,362-383`) move from ISO strings to integers.

**Blast radius.** Three crates: `db/`, `shared/`, `frontend/`. Touches the
landing sort comparator, the table renderer, and two fixture sets.

**Pros.** "Correct" end state — the wire type matches the storage type, numeric
sort is unambiguous, downstream consumers (a future combined stats timeline,
device-sync cursors) get an `i64` they can compute on directly. Removes the
fragile "this only sorts right because ISO is fixed-width" footgun entirely.

**Cons.** Largest surface, and it lands the frontend display-formatting decision
(locale? relative "3 days ago"? UTC ISO?) right now, which is really an Atrium/UX
question, not a DB-hygiene one. Heaviest conflict with concurrent
`projection.rs` / `sorting.rs` work. Bundles a UI change into a schema-hygiene
PR.

### Option C — Status quo + a read-time normalization helper (do not migrate columns)

**How it works.** Leave the columns TEXT. Add one Rust helper
`to_epoch(&str) -> i64` and use it everywhere a TEXT timestamp must be compared
to an INTEGER one (e.g. the F3.4 join).

**Migration shape.** None.

**Pros.** Zero migration risk; no table recreate.

**Cons.** Doesn't solve the problem — the SQL-level `GROUP BY date(...)`
non-uniformity (`docs/roadmap/3-4-stats.md`) is *in the query engine*, not in
Rust, so a Rust helper can't normalize a `GROUP BY` across mixed columns. Leaves
the lexicographic-sort footgun live. Every new TEXT-timestamp column (F3.2) makes
it worse. This is the "we'll regret it later" non-fix; recorded only for
completeness.

---

## Recommendation

**Adopt Option A.** Migrate all five DB columns to INTEGER unix-seconds via the
`0019` table-recreate, and format `added_at` / `modified` back to fixed-width ISO
on the `projection.rs` read path so `shared::EbookMetadata` and the entire
frontend stay byte-for-byte unchanged.

Rationale:

- It fixes the **actual** problem the roadmap cares about — the SQL-level type
  uniformity that lets F3.4 `GROUP BY date(col,'unixepoch')` across books and
  sessions, and lets F3.2's new `updated_at` columns be born INTEGER in a schema
  that's now uniformly INTEGER.
- It does so with the **smallest reversible surface**: DB-crate-only. The wire
  shape is unchanged, so the migration is not entangled with the
  not-yet-decided UI question of *how* to render a timestamp (relative vs.
  absolute vs. locale). Option B's wire-to-`i64` change is the strictly-better
  end state, but it should ride along with the Atrium Library reskin that owns
  date rendering — not block schema hygiene on a UX call.
- **Reversibility / cost of delay.** The *schema* change is forward-only and
  effectively irreversible once production rows convert (see Risks). But the
  *wire* decision under Option A is fully reversible: because the wire stays
  `String`, we can later flip to Option B's `i64` as an additive frontend PR
  without re-touching the schema. Choosing A now therefore does **not** foreclose
  B; choosing B now *does* foreclose deferring the UI decision. Delay cost is
  real: every new TEXT-timestamp column added before this lands (F3.2) is another
  column to recreate later.

The one invariant Option A must preserve and test: the ISO string emitted by
`strftime('%Y-%m-%dT%H:%M:%SZ', …, 'unixepoch')` is fixed-width and sorts
identically to the old `datetime('now')` output, so the landing lexicographic
sort (`sorting.rs` `cmp_with_missing_last`) keeps working.

---

## Migration plan (SKETCH — not to be applied)

`db/migrations/0019_unify_timestamps_to_unix_seconds.sql`, forward-only per
rule 06. The in-DDL `CAST(strftime('%s', col) AS INTEGER)` **is** the backfill —
no separate Rust boot hook is needed (unlike the `_norm` pattern), because the
conversion is pure SQL and every writer of these columns used
`datetime('now')` / `CURRENT_TIMESTAMP`, both of which produce
`'YYYY-MM-DD HH:MM:SS'` UTC that `strftime('%s', …)` parses exactly.

```sql
-- 0019_unify_timestamps_to_unix_seconds.sql  (forward-only)
-- foreign_keys is ON (init_db sets it). Wrap in the migrator's implicit txn.

-- books: timestamp + last_modified TEXT -> INTEGER. Recreate because the column
-- DEFAULT changes; ids are preserved via INSERT...SELECT so FK children and the
-- books_fts (0005) external-content index stay valid (no relink, no FTS rebuild).
CREATE TABLE books_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  uuid TEXT NOT NULL UNIQUE,
  library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  title TEXT NOT NULL COLLATE NOCASE,
  sort TEXT COLLATE NOCASE, author_sort TEXT COLLATE NOCASE,
  series_index REAL, pubdate TEXT,
  timestamp     INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  last_modified INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  has_cover INTEGER NOT NULL DEFAULT 0,
  description TEXT COLLATE NOCASE, isbn TEXT COLLATE NOCASE,
  accent_color TEXT,                 -- 0006
  title_norm TEXT, author_norm TEXT  -- 0016
);
INSERT INTO books_new SELECT
  id, uuid, library_id, path, title, sort, author_sort, series_index, pubdate,
  CAST(strftime('%s', timestamp)     AS INTEGER),
  CAST(strftime('%s', last_modified) AS INTEGER),
  has_cover, description, isbn, accent_color, title_norm, author_norm
FROM books;
DROP TABLE books;
ALTER TABLE books_new RENAME TO books;
-- Recreate EVERY books index from 0002/0006/0016: idx_books_uuid, _sort,
-- _author_sort, _series_index, _last_modified, _timestamp, _library_id,
-- idx_books_accent_null (0006), idx_books_norm (0016).
-- Confirm the books_fts triggers (0005) and re-create them if the recreate
-- dropped them; the bulk INSERT...SELECT into books_new fires no AFTER triggers
-- on `books`, but DROP TABLE books may cascade-drop FTS triggers — verify.

-- metadata_overrides.updated_at TEXT -> INTEGER (recreate).
CREATE TABLE metadata_overrides_new (
  book_uuid TEXT NOT NULL PRIMARY KEY,
  overrides TEXT NOT NULL DEFAULT '{}',
  has_cover_override INTEGER NOT NULL DEFAULT 0,
  updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
  updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
INSERT INTO metadata_overrides_new SELECT book_uuid, overrides, has_cover_override,
  updated_by, CAST(strftime('%s', updated_at) AS INTEGER) FROM metadata_overrides;
DROP TABLE metadata_overrides;
ALTER TABLE metadata_overrides_new RENAME TO metadata_overrides;

-- author_photos.fetched_at TEXT -> INTEGER (recreate; preserve both CHECKs).
CREATE TABLE author_photos_new (
  author_id INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE,
  source TEXT NOT NULL CHECK (source IN ('manual','openlibrary','letter')),
  url TEXT, bytes BLOB, mime TEXT,
  fetched_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  CHECK ((source='letter' AND bytes IS NULL AND mime IS NULL)
      OR (source<>'letter' AND bytes IS NOT NULL AND mime IS NOT NULL))
);
INSERT INTO author_photos_new SELECT author_id, source, url, bytes, mime,
  CAST(strftime('%s', fetched_at) AS INTEGER) FROM author_photos;
DROP TABLE author_photos; ALTER TABLE author_photos_new RENAME TO author_photos;

-- merge_log.merged_at TEXT -> INTEGER; undone_at TEXT -> INTEGER (nullable).
CREATE TABLE merge_log_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target_book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  source_uuid TEXT NOT NULL, source_metadata TEXT NOT NULL,
  merged_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
  merged_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
  undone_at INTEGER
);
INSERT INTO merge_log_new SELECT id, target_book_id, source_uuid, source_metadata,
  merged_by, CAST(strftime('%s', merged_at) AS INTEGER),
  CASE WHEN undone_at IS NULL THEN NULL ELSE CAST(strftime('%s', undone_at) AS INTEGER) END
FROM merge_log;
DROP TABLE merge_log; ALTER TABLE merge_log_new RENAME TO merge_log;
CREATE INDEX idx_merge_log_target ON merge_log(target_book_id);

-- ignored_authors.ignored_at DATETIME -> INTEGER (recreate).
CREATE TABLE ignored_authors_new (
  name TEXT NOT NULL PRIMARY KEY COLLATE NOCASE,
  ignored_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
INSERT INTO ignored_authors_new SELECT name, CAST(strftime('%s', ignored_at) AS INTEGER)
FROM ignored_authors;
DROP TABLE ignored_authors; ALTER TABLE ignored_authors_new RENAME TO ignored_authors;
```

No boot hook in `normalize.rs` / `pool.rs` is required. If we ever discover a
writer that stored a non-`datetime('now')` value, we'd add an idempotent
`backfill_*` next to `backfill_norm_columns` (`db/src/normalize.rs:53`, called
from `db/src/pool.rs:56`) — but none exists today, so the in-DDL CAST suffices.

---

## Affected code (Option A)

**Migration (new):** `db/migrations/0019_unify_timestamps_to_unix_seconds.sql`.

**Write sites — `datetime('now')` → `strftime('%s','now')`:**

- `db/src/sync/books.rs` `last_modified = datetime('now')` (~509).
- `db/src/sync/audiobooks.rs` same (~524).
- `db/src/merge/undo.rs` `UPDATE merge_log SET undone_at = datetime('now')`
  (~77) and `recreate_source_row` `COALESCE(?, datetime('now'))` (~120).
- `db/src/merge/transaction.rs` `metadata_overrides` upsert `updated_at`
  (~396,401).
- `db/src/metadata_overrides/upsert.rs` INSERT `updated_at` (~52,58).
- `db/src/author_photos_data.rs` `upsert_author_photo` `fetched_at` (~107).

**Read / round-trip sites:**

- `db/src/merge/snapshot.rs` `SourceSnapshot.timestamp: Option<String>` →
  `Option<i64>` (line 22); the `SELECT b.timestamp` (~95) now yields INTEGER;
  serde round-trip field type follows.
- `db/src/author_photos_data.rs` `author_photo_status` SELECT tuple
  `(String, String)` → `(String, i64)`, or simply drop `fetched_at` from the
  SELECT — every caller ignores it (`db/src/author_photos_data.rs:86-91`;
  callers use only `source` at `cascade.rs:90,148`).
- `db/src/merge/undo.rs` `undone_at` presence check is unchanged —
  `Option<i64>::is_some()` still gates `AlreadyUndone`.
- `db/src/covers.rs` `get_last_modified_epoch` (~246): replace
  `CAST(strftime('%s', last_modified) AS INTEGER)` with a plain
  `SELECT last_modified` (now already INTEGER).

**Read site — DB-side ISO formatting (the Option-A keystone):**

- `db/src/books/projection.rs` `BOOK_COLUMNS` (line 33): wrap `b.last_modified`
  / `b.timestamp` in `strftime('%Y-%m-%dT%H:%M:%SZ', col, 'unixepoch') AS …` so
  `build_metadata` (lines 176,188) still reads `String`.

**Unchanged under Option A:** `shared/src/ebook/metadata.rs`,
`frontend/src/pages/landing/{sorting,table,filtering}.rs`. (These are the files
Option B would change.)

---

## Test plan

Per rule 03: sibling `<mod>/tests.rs`, `sqlite::memory:` via `init_db` (which
runs all migrations including `0019`), happy + per-variant.

- **Column type post-migration.** In a migration/`pool.rs` test: insert a row
  and assert the stored value is an INTEGER unix-second for each migrated column
  — `books.timestamp`, `books.last_modified`, `metadata_overrides.updated_at`,
  `author_photos.fetched_at`, `merge_log.merged_at`, `ignored_authors.ignored_at`
  (fetch as `i64`, assert `> 0`; or `SELECT typeof(col) = 'integer'`).
- **Backfill conversion (the acceptance test that MUST fail on the old schema).**
  A `sqlite::memory:` DB running `0019` immediately can't hold pre-migration TEXT
  rows, so lock the conversion *semantics* directly:
  `SELECT CAST(strftime('%s','2024-01-02 03:04:05') AS INTEGER)` must equal
  `1704164645`. This is the conversion the old TEXT rows depend on; it is the
  parsing case rule 03 says to cover. Pair it with a test that inserts via the
  new default and reads back a plausible "now" epoch.
- **`projection.rs` (Option-A keystone).** A `list_books` / `get_book` test
  asserting `added_at` / `modified` are well-formed fixed-width ISO strings
  (`YYYY-MM-DDTHH:MM:SSZ`) and that two known rows sort correctly under the
  landing lexicographic comparator — i.e. the sort contract that the old TEXT
  columns provided still holds.
- **`merge/snapshot.rs` + `undo.rs`.** Existing merge/undo round-trip tests pass
  with `timestamp: Option<i64>`; add one asserting a merged-then-undone book's
  restored `timestamp` is a positive integer and `undone_at` presence still
  gates `AlreadyUndone`.
- **`author_photos_data.rs`.** `author_photo_status` happy path still returns the
  right `source` after the tuple-type change.
- **`covers.rs`.** `get_last_modified_epoch` returns `Some(positive i64)` for an
  indexed book and `None` for a missing id (re-verify existing tests after the
  query simplifies).
- Run: `cargo test -p omnibus-db`, `cargo test -p omnibus-frontend --features
  server`, `cargo test -p omnibus-shared`, `cargo test -p omnibus` (`just test`),
  then `just dev-bounce` so `0019` applies to the live dev DB.

(Under Option B, additionally: replace ISO-string fixtures in `sorting.rs`
281-302 / 362-383 and `filtering.rs` with `i64`, and assert `NewestAdded` /
`LastUpdated` order numerically incl. missing-last behavior.)

---

## Risks & rollback

- **Forward-only, fix-forward (rule 06).** There is no down-migration. A mistake
  in `0019` is corrected by a `0020`, never by editing `0019` once it has run
  anywhere (sqlx checksums it).
- **Data-loss surface = the table recreate of `books`.** `books` is FK-parent to
  `book_files` and the link tables and is external-content for `books_fts`
  (0005). The `INSERT…SELECT … id` preserves primary keys, so children stay
  linked and FTS stays valid — *provided the column list is exact*. The single
  biggest hazard is silently dropping a `books` column added by an intervening
  migration (e.g. `accent_color` from 0006, the `_norm` columns from 0016): the
  recreate must enumerate the **current** `books` shape, not the 0002 shape.
  Verify against the live schema at authoring time.
- **Irreversible once data accumulates.** After production rows convert to
  INTEGER, the original TEXT strings are gone (only the second-resolution epoch
  remains — which is all `datetime('now')` ever carried, so no precision is
  lost). But you cannot un-convert without another migration and you cannot
  recover the exact original string formatting.
- **FTS triggers.** If `books_fts` (0005) installs AFTER triggers on `books`,
  `DROP TABLE books` may drop them; the recreate must re-establish them. Confirm
  during authoring and add to the migration if so — a missing FTS trigger
  silently breaks search on subsequently-indexed books.
- **Mitigation.** Author and run `0019` against a *copy* of a real
  Calibre-derived DB (not just `sqlite::memory:`, which starts empty) to exercise
  the CAST path on actual `datetime('now')` rows before merge.

---

## Sequencing & dependencies

This is a **big mechanical surface** that conflicts with most other db-review
PRs — the scout spec lists `conflictsWith: F1, F2, F3, F7, F8, F10, F17, F18`.
The collisions are concentrated in `db/src/sync/books.rs` and
`db/src/books/projection.rs`, which several findings also edit.

Recommended ordering:

- **Land this AFTER the `books`-identity / projection-shape findings settle**, or
  the `books_new` column list and the `BOOK_COLUMNS` edits will need rework. In
  particular the F1 ↔ F2 ↔ F10 chain (book-identity / merge / override-orphan
  reconcile) all touch `books` and `merge_log`; sequencing F11 last among the
  `books`-table migrations means `0019` recreates the *final* column set once,
  rather than chasing intermediate shapes.
- If F11 must go first, keep `0019` purely additive-in-spirit (type-only
  conversion, no column add/drop) so a later `books` migration rebases cleanly on
  top.
- **Independent of the index findings.** The "session indexes don't support
  stats" finding adds `(user_id, started_at)` indexes to the session tables — no
  overlap with the five tables here, so it can land in parallel.
- **Unblocks F3.4 / F3.2** rather than depending on them: those features want a
  uniformly-INTEGER timestamp space, so this should precede their schema work.

---

## Open questions

1. **D1 final call.** Confirm Option A (wire stays `String`) vs. Option B (wire
   `i64`). The recommendation is A; B is a defensible "do it once" if the Atrium
   Library reskin that owns date rendering is landing concurrently and wants the
   `i64`.
2. **`books_fts` trigger fate on recreate.** Does 0005 define AFTER
   insert/update/delete triggers on `books`? If yes, `0019` must drop+recreate
   them. (Verify at authoring time; gates a correct migration.)
3. **Books-table migration ordering.** Does any of F1/F2/F10 add or remove a
   `books` column before F11 lands? If so, F11's `books_new` column list must be
   authored against that later shape — coordinate the merge order.
4. **`merge_log.source_metadata` JSON.** The snapshot blob round-trips
   `b.timestamp` (`db/src/merge/snapshot.rs:22`). Confirm no *persisted* merge
   snapshot JSON already on disk encodes `timestamp` as a string that would
   deserialize-fail once the field becomes `Option<i64>`. If old snapshots exist,
   add a serde compatibility shim or a one-time snapshot rewrite.
5. **Display formatting (only if B).** If Option B, where does the epoch→human
   formatting live and what format (UTC ISO, locale, relative)? That is an
   Atrium/UX decision, not a DB one — another reason A defers it cleanly.
