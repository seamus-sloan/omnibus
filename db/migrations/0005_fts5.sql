-- F0.4: FTS5 full-text index over books + taxonomy.
--
-- Standalone vtable (no `content=` option): this table stores the indexed
-- text inside the FTS index itself. We use standalone (not external-content
-- or contentless) because three of the indexed columns (authors/series/tags)
-- are denormalized from joined taxonomy tables:
--   - external-content FTS5 is not a fit: its 'rebuild' path reads columns
--     directly from one named content table and cannot reconstruct joined
--     values.
--   - contentless (content='') supports only inserts; DELETE/UPDATE require
--     `contentless_delete`, which we can't rely on across all SQLite builds.
-- Standalone's storage overhead is a few KB per book — negligible for a
-- library database.
--
-- Write path: `replace_books` writes the denormalized row inline in the same
-- transaction as the book + link inserts. That keeps the bulk reindex free
-- of per-row trigger fan-out across six tables.
--
-- Rename path: the three triggers below propagate UPDATEs to author/tag/
-- series names into books_fts, since those happen outside `replace_books`.

CREATE VIRTUAL TABLE books_fts USING fts5(
    title,
    authors,
    series,
    tags,
    description,
    isbn,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix   = '2 3'
);

-- Backfill existing rows. Harmless on fresh installs (empty books).
INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
SELECT
    b.id,
    b.title,
    (SELECT group_concat(a.name, ' ')
     FROM books_authors_link l JOIN authors a ON a.id = l.author
     WHERE l.book = b.id),
    (SELECT group_concat(s.name, ' ')
     FROM books_series_link  l JOIN series  s ON s.id = l.series
     WHERE l.book = b.id),
    (SELECT group_concat(t.name, ' ')
     FROM books_tags_link    l JOIN tags    t ON t.id = l.tag
     WHERE l.book = b.id),
    b.description,
    b.isbn
FROM books b;

-- Rename propagation. UPDATE OF name only fires when the column actually
-- changes, so simple INSERT/UPDATE-with-same-name churn doesn't touch FTS.

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
