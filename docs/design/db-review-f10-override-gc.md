# F10 — Missing-files GC

Status: Implemented (2026-06-28). Supersedes the earlier "Option C soft-detach
of orphan `metadata_overrides`" decision — that rested on a pre-F2 model where
reindex/library-removal *deleted* `books` rows. Post-F2 they don't, so override
rows don't orphan; the real problem is fileless-`books` accumulation. This doc
records the design as built.

---

## Problem (post-F2)

Two F2 behaviors removed the orphan-override problem the original F10 framed,
and created a different one:

- **Library removal is never-prune.** `prune_orphan_libraries`
  (`db/src/settings.rs`) keeps a removed root's books (and their overrides +
  soft-ref user data); only *childless* `scan_roots` are swept. A re-added or
  repointed path re-links every book by its relative `scan_key`, uuid intact.
- **File removal ghosts, it doesn't delete.** `mark_book_files_missing`
  (`db/src/sync/books/shared.rs`, the renamed `ghost_book_by_id`) drops a
  removed file's `book_files`/links/FTS but **retains** the `books` row, its
  `uuid`, `metadata_overrides`, and every soft-ref user-data row, so a returning
  file re-attaches to the same uuid (durable user data).

So `metadata_overrides` essentially never orphans (only a merge hard-deletes a
book, already handled at `db/src/merge/transaction.rs`). Instead, **fileless
`books` rows accumulate forever** — `sync_removed` even logged the count and
deferred their GC to F10. Without a bound, every transiently- or
permanently-removed file leaves a row behind.

---

## Decisions

- **GC target:** purge `books` rows that have been **missing their files** past
  a retention window — not orphan override rows.
- **User-data safety:** **never purge a book that carries user data** (a row in
  any 0027 soft-ref table: `reading_progress`, `bookmarks`, `reading_sessions`,
  `listening_sessions`, `highlights`). Those are kept indefinitely (one cheap
  row). Only user-data-free missing books are purged. No soft-detach /
  `detached_at` machinery; "unlinked annotations" UX stays deferred to F3.2.
- **Overrides** (regenerable) are hard-deleted **with** the book, plus their
  cover files.
- **Retention:** **30 days** missing before purge-eligible
  (`MISSING_FILES_RETENTION_DAYS`).
- **When:** best-effort and logged, after each reindex (ebook + audiobook).
  There is no explicit-library-removal arm — removal is never-prune.
- **Wishlist exemption:** `is_missing_files_override = 1` rows are
  intentionally fileless (forward-compat for a future "wishlist") and are never
  flagged-missing or GC'd.

---

## Schema (migration 0029)

`0029_books_missing_files.sql` adds three columns to `books` (additive,
forward-only per rule 06):

| Column | Type | Meaning |
|---|---|---|
| `is_missing_files` | `INTEGER NOT NULL DEFAULT 0` | 1 once every file is gone; the authoritative read/GC flag. |
| `missing_files_since` | `INTEGER` (nullable) | unix-epoch seconds of the first ghosting; drives the retention age (`unixepoch()`). NULL while attached. |
| `is_missing_files_override` | `INTEGER NOT NULL DEFAULT 0` | 1 = intentionally fileless (wishlist); exempt from missing-flagging and GC. |

Plus a partial index `idx_books_missing_files ON books(missing_files_since)
WHERE is_missing_files = 1 AND is_missing_files_override = 0` for the GC scan.

**Boot backfill** (`missing_files::backfill_missing_files_flags`, called from
`init_db`): stamps pre-migration fileless rows so their clock starts at boot.
Idempotent (`is_missing_files = 0` + `NOT EXISTS book_files` guard), a no-op
once caught up — the `normalize::backfill_norm_columns` pattern.

---

## How it works

- **Flag set:** `mark_book_files_missing` stamps `is_missing_files = 1,
  missing_files_since = unixepoch()` (guarded on `is_missing_files = 0` so the
  original `since` survives a re-mark). Shared by ebook and audiobook removed
  buckets.
- **Flag clear:** the file-write chokepoints —
  `clear_missing_files_flag`, called from ebook `insert_book_file_row` and the
  audiobook `insert_audiobook_file_row` — reset the flag when a book regains a
  file (no-op on a fresh insert).
- **GC:** `gc_books_missing_files(pool, retention_days)` selects books that are
  missing, non-override, past retention, **and** have no `book_files`, no
  `merged_uuids` attachment anchor, and no soft-ref user-data row; then deletes
  their `metadata_overrides` + `books` rows (chunked at 500) and unlinks their
  cover + override-cover files off the runtime. Returns the purged count.
  `NOT EXISTS book_files` is the authoritative guard, so a stale flag can never
  cause a wrong purge.
- **Read path:** ghosts are already hidden from browse/discovery (links/FTS
  wiped). The override-creators/series *arms* in `browse.rs` /
  `discovery/authors.rs` join `metadata_overrides` without going through links,
  so they gained `AND b.is_missing_files = 0` to keep a missing book with a
  creators/series override from surfacing.

---

## Affected code

| File | Change |
|---|---|
| `db/migrations/0029_books_missing_files.sql` | NEW columns + partial index |
| `db/src/missing_files.rs` (+ `tests.rs`) | NEW `gc_books_missing_files`, `backfill_missing_files_flags`, `MissingFilesError`, `MISSING_FILES_RETENTION_DAYS` |
| `db/src/lib.rs`, `db/src/pool.rs` | module + re-exports; `init_db` backfill call + `InitDbError::MissingFiles` |
| `db/src/sync/books/shared.rs` | `ghost_book_by_id` → `mark_book_files_missing` (sets flag) + `clear_missing_files_flag` on file write |
| `db/src/sync/books/{mod,removed}.rs`, `db/src/sync/audiobooks.rs` | rename + flag clear at the audiobook file-write chokepoint |
| `db/src/indexer.rs` | `gc_missing_files_best_effort` after each reindex |
| `db/src/browse.rs`, `db/src/discovery/authors.rs` | `is_missing_files = 0` guard on the override arms |

---

## Test plan (`db/src/missing_files/tests.rs`)

Per rule 03 (`sqlite::memory:`, long names). GC predicate: purge past retention
with no user data (acceptance); keep within window; keep with reading progress;
keep with files; keep with a `merged_uuids` attachment; keep with the wishlist
override; delete override row + cover on purge; `Db` error on a closed pool;
backfill idempotent. Sync-wired lifecycle: a removed file sets the flag; a
returning file clears it and preserves the override; a library repoint
round-trip preserves identity + override; and the documented path-identity edge
(an identical relative path in a repointed dir keeps one book and smears the
override — see Open).

---

## Risks & sequencing

- Forward-only migration (rule 06); columns nullable/defaulted, no down-migration.
- GC is global + idempotent; running it after each per-library reindex is cheap.
- Best-effort: a GC error is logged and never aborts a reindex.
- **F3.2 ratings/journals**, when added, become soft-ref user-data tables and
  **must** be added to the GC's user-data `NOT EXISTS` guard list (noted in
  `docs/roadmap/3-2-ratings-journaling.md`).

## Open

- **Path-based identity (revisit under F2).** A book is identified by `(library
  slot, relative scan_key)`, not content. Repointing a slot to a directory whose
  file sits at the same relative path adopts the prior book's identity, so an
  override smears onto the new physical file rather than two distinct books being
  recognized. Documented by a test; flagged in
  `docs/design/db-review-f2-stable-uuid-identity.md` to revisit under
  content-based identity.
