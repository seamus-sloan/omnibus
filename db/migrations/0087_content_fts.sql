-- Full-text index over EPUB book content, per chapter (#2282).
--
-- `book_content_chapters` holds one row per (book_uuid, spine_index) with the
-- stripped chapter text plus the `(mtime_epoch, size_bytes)` snapshot of the
-- `book_files` row the text was extracted from — the same pair the reindex
-- diff keys on (rule 09), so "the file changed" and "the text index is stale"
-- are one statement. `book_uuid` is a soft reference (no FK, no cascade):
-- unlike durable user data this index is regenerable, so instead of a cascade
-- the content-FTS worker pass heals it — pruning rows whose uuid no longer
-- resolves to a book and re-extracting rows whose snapshot no longer matches
-- the served file (`db/src/content_fts/`).
--
-- `book_content_fts` is an external-content FTS5 table over it. `books_fts`
-- (0005) is standalone because its columns denormalize joined taxonomy rows,
-- which external content cannot rebuild — but chapter text has exactly one
-- source row here, and at this table's grain (chapter prose, not metadata
-- fields) storing the text once instead of twice is material. The two tables
-- stay separate: different row grains, and mixing them would break the
-- existing metadata bm25 ranking. No prefix index — content search is not a
-- type-ahead surface, and prefix queries still work via the trailing `*`.
--
-- The index lives in the main sqlite DB with no cap/eviction knob: its
-- on-disk cost over a representative library is unmeasured (AC6 of #2282
-- remains open), so no knob is invented ahead of that measurement.

CREATE TABLE book_content_chapters (
    id          INTEGER PRIMARY KEY,
    book_uuid   TEXT    NOT NULL,
    spine_index INTEGER NOT NULL,
    mtime_epoch INTEGER NOT NULL,
    size_bytes  INTEGER NOT NULL,
    text        TEXT    NOT NULL,
    UNIQUE (book_uuid, spine_index)
);

CREATE VIRTUAL TABLE book_content_fts USING fts5(
    text,
    content       = 'book_content_chapters',
    content_rowid = 'id',
    tokenize      = 'unicode61 remove_diacritics 2'
);

-- The standard external-content contract: every write to the content table
-- is mirrored into the index, so the two can never drift.

CREATE TRIGGER book_content_fts_ai AFTER INSERT ON book_content_chapters
BEGIN
    INSERT INTO book_content_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER book_content_fts_ad AFTER DELETE ON book_content_chapters
BEGIN
    INSERT INTO book_content_fts(book_content_fts, rowid, text)
    VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER book_content_fts_au AFTER UPDATE ON book_content_chapters
BEGIN
    INSERT INTO book_content_fts(book_content_fts, rowid, text)
    VALUES ('delete', old.id, old.text);
    INSERT INTO book_content_fts(rowid, text) VALUES (new.id, new.text);
END;
