-- Book-uuid-first indexes for the five older user-data tables.
--
-- `gc_books_missing_files` (db/src/missing_files.rs) probes each of these
-- tables with `NOT EXISTS (SELECT 1 FROM <t> WHERE book_uuid = b.uuid)`
-- on every reindex cycle. Their existing `(user_id, book_uuid)` indexes
-- can't serve the probe — SQLite skips a multi-column index when the
-- leading column (`user_id`) isn't in the predicate — so each subquery
-- degrades to a full-table scan. `merge/transaction.rs::move_progress_and_history`
-- runs `UPDATE`/`DELETE ... WHERE book_uuid = ?` on the same tables and
-- pays the same cost. `user_ratings` already ships this fix (0031); the
-- five older siblings never got it.
CREATE INDEX IF NOT EXISTS idx_reading_progress_book_uuid   ON reading_progress(book_uuid);
CREATE INDEX IF NOT EXISTS idx_bookmarks_book_uuid          ON bookmarks(book_uuid);
CREATE INDEX IF NOT EXISTS idx_reading_sessions_book_uuid   ON reading_sessions(book_uuid);
CREATE INDEX IF NOT EXISTS idx_listening_sessions_book_uuid ON listening_sessions(book_uuid);
CREATE INDEX IF NOT EXISTS idx_highlights_book_uuid         ON highlights(book_uuid);
