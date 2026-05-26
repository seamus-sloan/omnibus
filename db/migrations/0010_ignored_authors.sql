-- Reindex-resurrection guard for the "Delete author" admin flow
-- (issue #159).
--
-- Without this table, deleting a junk-author row (e.g. Calibre's
-- `calibre (8.x.x) [https://...]` tooling-metadata contributor) is undone
-- by the very next `indexer::reindex`: it walks every EPUB's OPF, sees
-- the bad name as a `<dc:contributor>`, and `resolve_or_insert_author`
-- silently re-creates the row + relinks the book to it.
--
-- `resolve_or_insert_author` consults this table before its INSERT — a
-- hit returns `None` and the caller (`insert_metadata_links`) simply
-- skips that contributor. The admin Delete-author flow is the only
-- writer; recovery is a plain `DELETE FROM ignored_authors WHERE name = ?`
-- followed by a reindex.

CREATE TABLE ignored_authors (
    name       TEXT     NOT NULL PRIMARY KEY COLLATE NOCASE,
    ignored_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
