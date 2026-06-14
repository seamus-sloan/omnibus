-- Schema cleanup: drop dead and redundant objects. Forward-only; every
-- statement is `IF EXISTS` so a fresh DB and an upgraded DB converge.
--
-- F17 — four indexes that duplicate a UNIQUE/PK auto-index or are a strict
-- prefix of a composite. SQLite serves an equality/range probe from the
-- leading columns of a composite index, so the narrower duplicate carries
-- no read benefit and only adds write amplification:
--   idx_books_uuid              — books.uuid is UNIQUE (0002), which builds
--                                 its own auto-index; the explicit copy is
--                                 redundant.
--   idx_book_files_book_id      — strict prefix of idx_book_files_book_format
--                                 (book_id, format) (0018).
--   idx_book_file_parts_lookup  — duplicates the UNIQUE(book_file_id, ordinal)
--                                 auto-index (0014).
--   reading_progress_user_book_idx — leftmost prefix of the
--                                 UNIQUE(user_id, book_id, format) auto-index
--                                 (0013).
-- The bookmarks/reading_sessions/listening_sessions (user_id, book_id)
-- indexes from 0013 are deliberately kept: those tables have no covering
-- UNIQUE, so they are not redundant.
DROP INDEX IF EXISTS idx_books_uuid;
DROP INDEX IF EXISTS idx_book_files_book_id;
DROP INDEX IF EXISTS idx_book_file_parts_lookup;
DROP INDEX IF EXISTS reading_progress_user_book_idx;

-- F18 — speculative partial index added in 0006 for a "backfill missing
-- accents" worker that was never built. No query filters on
-- `accent_color IS NULL`; the only real consumers use IS NOT NULL
-- (db/src/browse.rs), which a NULL-only partial index cannot serve. It only
-- adds write amplification.
DROP INDEX IF EXISTS idx_books_accent_null;

-- F20 — vestigial single-row `app_state` table from the pre-normalization
-- counter-app placeholder, seeded with value 0 in 0001 and never read or
-- written by any code. The `settings` KV table covers any real global-state
-- need.
DROP TABLE IF EXISTS app_state;
