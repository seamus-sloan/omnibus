-- Add `genres` to `books_fts` so search can see the one taxonomy it couldn't.
--
-- FTS5 has no `ALTER TABLE … ADD COLUMN`, so the column arrives by
-- create-replacement: build a seven-column twin, copy every row across, drop
-- the old table, rename the twin into its place.
--
-- The copy reads `books_fts` rather than re-deriving from `books`, and that
-- is the point: `overlay_overrides` writes user title/author/series/tag edits
-- on top of the canonical row, and a re-derive would silently revert every
-- one of them in the index until the next reindex.
--
-- The three rename triggers from 0005_fts5.sql name `books_fts` in their
-- bodies, so they cannot survive the swap — dropped first, recreated verbatim
-- after.
--
-- Genres are the only indexed column with no canonical table behind them:
-- they live solely in `metadata_overrides.overrides -> '$.genres'` (0066), so
-- the backfill reads the override JSON directly. A book with no override, or
-- one whose override never set the key, yields no `json_each` rows and lands
-- on the empty string.

DROP TRIGGER IF EXISTS books_fts_authors_rename;
DROP TRIGGER IF EXISTS books_fts_tags_rename;
DROP TRIGGER IF EXISTS books_fts_series_rename;

CREATE VIRTUAL TABLE books_fts_new USING fts5(
    title,
    authors,
    series,
    tags,
    description,
    isbn,
    genres,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix   = '2 3'
);

INSERT INTO books_fts_new(rowid, title, authors, series, tags, description, isbn, genres)
SELECT
    f.rowid,
    f.title,
    f.authors,
    f.series,
    f.tags,
    f.description,
    f.isbn,
    COALESCE((SELECT group_concat(je.value, ' ')
              FROM books b
              JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              JOIN json_each(mo.overrides, '$.genres') je
             WHERE b.id = f.rowid
               AND json_type(mo.overrides, '$.genres') IS NOT NULL), '')
FROM books_fts f;

DROP TABLE books_fts;
ALTER TABLE books_fts_new RENAME TO books_fts;

-- Recreated verbatim from 0005_fts5.sql.

CREATE TRIGGER books_fts_authors_rename AFTER UPDATE OF name ON authors
BEGIN
    UPDATE books_fts SET authors = (
        SELECT group_concat(a.name, ' ')
        FROM books_authors_link l JOIN authors a ON a.id = l.author
        WHERE l.book = books_fts.rowid
    )
    WHERE rowid IN (SELECT book FROM books_authors_link WHERE author = NEW.id);
END;

CREATE TRIGGER books_fts_tags_rename AFTER UPDATE OF name ON tags
BEGIN
    UPDATE books_fts SET tags = (
        SELECT group_concat(t.name, ' ')
        FROM books_tags_link l JOIN tags t ON t.id = l.tag
        WHERE l.book = books_fts.rowid
    )
    WHERE rowid IN (SELECT book FROM books_tags_link WHERE tag = NEW.id);
END;

CREATE TRIGGER books_fts_series_rename AFTER UPDATE OF name ON series
BEGIN
    UPDATE books_fts SET series = (
        SELECT group_concat(s.name, ' ')
        FROM books_series_link l JOIN series s ON s.id = l.series
        WHERE l.book = books_fts.rowid
    )
    WHERE rowid IN (SELECT book FROM books_series_link WHERE series = NEW.id);
END;
