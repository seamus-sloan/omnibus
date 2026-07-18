-- Per-file attachment identity: give each `book_files` row its own
-- relative-path `scan_key`.
--
-- Attachments (cross-format merges, and multi-part audiobook merges) were
-- keyed on the `(book_id, format)` slot — one file per format per book. That
-- makes N same-format files under one book (a manually-merged multi-part
-- `.m4b` set) impossible to hold stably: every scan resurrects all but one
-- part as a standalone duplicate. Keying each `book_files` row on the file's
-- own `scan_key` (the library-relative path — same value the reindex diff and
-- `merged_uuids` already match on) lets N distinct files coexist under one
-- book, each matched to its own `merged_uuids` guard row.
--
-- Additive and non-destructive: a nullable column. The boot backfill
-- (`identity::backfill_book_files_scan_keys`) fills `scan_key` for
-- pre-migration rows from `book_file_parts` (audiobooks) and
-- `merged_uuids` / `books.scan_key` (ebooks) — no filesystem reads —
-- mirroring `normalize::backfill_norm_columns`.

ALTER TABLE book_files ADD COLUMN scan_key TEXT;

-- The hot reindex query `list_merged_rows_for_formats` joins
-- `book_files` to `merged_uuids` on `(book_id, scan_key)`; index it so the
-- per-file join stays cheap as attachment count grows.
CREATE INDEX IF NOT EXISTS idx_book_files_book_scan
    ON book_files(book_id, scan_key);
