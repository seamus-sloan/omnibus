-- #949 — persist a completed format conversion as its own `book_files` row,
-- coexisting with the original scanned format.
--
-- `book_files` already allows N rows per `(book_id, format)` — multi-part
-- audiobooks rely on that (0043 dropped the old `UNIQUE(book_id, format)`
-- constraint) — so a converter-written row cannot lean on that shape for
-- idempotency: re-running the same conversion must update its own row, not
-- collide with (or duplicate alongside) an unrelated same-format file.
--
-- `origin` tags which subsystem wrote a row. NULL (the default, left
-- unbackfilled) means "the scanner", matching every existing row. The
-- converter writes 'converted', and the partial unique index below scopes
-- uniqueness to just that origin — one converted row per `(book_id, format)`,
-- with no constraint on how many scanned rows share the same format.
ALTER TABLE book_files ADD COLUMN origin TEXT;

CREATE UNIQUE INDEX idx_book_files_converted_unique
    ON book_files(book_id, format)
    WHERE origin = 'converted';
