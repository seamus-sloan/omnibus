-- Chapter markers extracted from audiobook containers.
--
-- M4B files embed chapter atoms (Nero chpl or QuickTime chapter track).
-- MP3 files may carry ID3v2 CHAP/CTOC frames. Multi-file books without
-- embedded chapters get one synthetic row per part so the frontend can
-- always assume file_chapters.len() >= 1.
--
-- start_seconds is absolute (from book start), not relative to a part.

CREATE TABLE file_chapters (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    book_file_id     INTEGER NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    ordinal          INTEGER NOT NULL,
    title            TEXT    NOT NULL DEFAULT '',
    start_seconds    REAL    NOT NULL DEFAULT 0,
    duration_seconds REAL    NOT NULL DEFAULT 0,
    UNIQUE(book_file_id, ordinal)
);

CREATE INDEX idx_file_chapters_book_file ON file_chapters(book_file_id);
