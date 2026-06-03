-- 0014_audiobook_parts.sql
--
-- F2.3 multi-file audiobook support. A "book" in the audiobook library can
-- be either a single audio file (m4b / m4a / single mp3) or a folder of
-- per-chapter mp3s. The basic player merged in #327 treated each file as
-- its own book; this migration introduces a part-list table so a single
-- `book_files` row can fan out across N source files.
--
-- Backfill: every existing audiobook `book_files` row gets exactly one
-- part at ordinal 0 pointing at its own filename. After the migration the
-- read path is uniform — every audiobook has at least one part.

CREATE TABLE book_file_parts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    book_file_id     INTEGER NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    -- Playlist ordering. Sourced from the ID3 `track` tag; filename is the
    -- tiebreaker so rip tools that emit identical track tags still produce
    -- a stable order.
    ordinal          INTEGER NOT NULL,
    -- Path-relative-to-`books.path`. Same shape as `book_files.filename`
    -- (without extension) but kept as a full filename here because the
    -- parts can have different extensions in mixed-format folders and the
    -- HLS pipeline needs the absolute on-disk path.
    filename         TEXT    NOT NULL,
    size_bytes       INTEGER NOT NULL,
    mtime_epoch      INTEGER NOT NULL DEFAULT 0,
    -- Set during indexing via lofty's `properties().duration()`. Required
    -- for HLS manifest math (total = sum across parts; segment count =
    -- ceil(total / target_segment_duration)).
    duration_seconds REAL    NOT NULL DEFAULT 0,
    UNIQUE(book_file_id, ordinal)
);

CREATE INDEX idx_book_file_parts_lookup ON book_file_parts(book_file_id, ordinal);

-- One-time backfill of every existing audiobook row. The single-file case
-- becomes a one-part group with ordinal 0; the multi-file case will be
-- regenerated on the next reindex (the scanner now groups folders into
-- one row + N parts). Ebook formats (EPUB) are left alone — no part rows.
--
-- `duration_seconds` defaults to 0 here; the indexer fills it in via
-- lofty on the next pass. Until then the HLS pipeline treats unknown
-- durations as needing a fresh probe before transcode.
INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch)
SELECT id, 0, filename, size_bytes, mtime_epoch
  FROM book_files
 WHERE format IN ('M4B', 'M4A', 'MP3');
