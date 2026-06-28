-- F10 — Missing-files GC: track books whose files have disappeared so a
-- bounded GC can purge the long-missing, user-data-free ones.
--
-- Post-F2 a removed file no longer deletes its `books` row: the row is retained
-- fileless (its `uuid`, `metadata_overrides`, and soft-ref user data survive)
-- so a returning file re-links by `scan_key`. Without a GC those fileless rows
-- accumulate forever (see `db/src/sync/books/removed.rs`). These columns let
-- `missing_files::gc_books_missing_files` find rows missing past a retention
-- window and purge the ones carrying no user data.
--
--   is_missing_files           1 once every file is gone — set by
--                              `mark_book_files_missing`, cleared when a file
--                              returns. The authoritative read/GC flag.
--   missing_files_since        unix-epoch seconds of the first ghosting; drives
--                              the retention age. NULL while attached.
--   is_missing_files_override  1 marks a book *intentionally* fileless (future
--                              "wishlist" entries) — never flagged-missing and
--                              never GC'd. Default 0; only the GC skip ships now.
--
-- Additive, nullable/defaulted, forward-only (rule 06). No in-SQL backfill:
-- `init_db` runs `backfill_missing_files_flags` to stamp pre-migration fileless
-- rows so their retention clock starts at boot time.

ALTER TABLE books ADD COLUMN is_missing_files          INTEGER NOT NULL DEFAULT 0;
ALTER TABLE books ADD COLUMN missing_files_since        INTEGER;
ALTER TABLE books ADD COLUMN is_missing_files_override  INTEGER NOT NULL DEFAULT 0;

-- Drives the GC scan: only the missing, non-override rows, keyed by age.
CREATE INDEX IF NOT EXISTS idx_books_missing_files
    ON books(missing_files_since)
    WHERE is_missing_files = 1 AND is_missing_files_override = 0;
