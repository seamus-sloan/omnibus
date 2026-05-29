-- Composite index for the BOOK_COLUMNS hot-path scalar subqueries.
--
-- `db/src/books.rs` (`BOOK_COLUMNS`, used by `list_books` / `get_book` /
-- `search_books`) selects each book's primary file via
-- `... WHERE bf.book_id = b.id ORDER BY (bf.format != 'EPUB'), bf.format LIMIT 1`.
-- The existing `idx_book_files_book_id` (0002) covers only the equality
-- filter, so SQLite performs a per-book in-memory sort to satisfy the
-- `ORDER BY format`. A composite `(book_id, format)` index lets a single
-- index scan cover both the filter and the ordering.
--
-- `idx_book_files_book_id` is intentionally retained: book_id-only lookups
-- elsewhere keep their dedicated index and dropping it adds behavioral risk
-- for no meaningful gain on this Low-severity change.
CREATE INDEX IF NOT EXISTS idx_book_files_book_format
  ON book_files(book_id, format);
