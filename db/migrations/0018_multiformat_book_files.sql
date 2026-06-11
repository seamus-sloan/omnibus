-- Allow multiple files of the same format per book (e.g. five M4B parts
-- merged into one logical audiobook, or two EPUB editions on one card).
--
-- Drops the UNIQUE(book_id, format) constraint from 0002 and replaces it
-- with UNIQUE(book_id, format, ordinal) so ordinals stay unique within a
-- format on a given book.
--
-- New columns:
--   ordinal  — sort order within a format (0-indexed, default 0)
--   label    — user-friendly display name ("Part 1 of 5", original title)

-- SQLite cannot ALTER TABLE … DROP CONSTRAINT, so recreate the table.
CREATE TABLE book_files_new (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    book_id      INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    format       TEXT    NOT NULL COLLATE NOCASE,
    filename     TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    mtime        TEXT    NOT NULL,
    mtime_epoch  INTEGER NOT NULL DEFAULT 0,
    library_path TEXT,
    path         TEXT,
    ordinal      INTEGER NOT NULL DEFAULT 0,
    label        TEXT,
    UNIQUE(book_id, format, ordinal)
);

INSERT INTO book_files_new
    (id, book_id, format, filename, size_bytes, mtime, mtime_epoch,
     library_path, path, ordinal, label)
SELECT id, book_id, format, filename, size_bytes, mtime, mtime_epoch,
       library_path, path, 0, NULL
  FROM book_files;

DROP TABLE book_files;
ALTER TABLE book_files_new RENAME TO book_files;

-- Recreate indices from 0002 + 0011.
CREATE INDEX idx_book_files_book_id     ON book_files(book_id);
CREATE INDEX idx_book_files_book_format ON book_files(book_id, format);
