-- F11 — unify machine timestamps on INTEGER unix-seconds.
--
-- Five tables still stored a "machine" timestamp as TEXT (`datetime('now')`)
-- or DATETIME (`CURRENT_TIMESTAMP`) while every table since the auth migration
-- uses INTEGER `strftime('%s','now')`. The divergence blocks a uniform
-- `GROUP BY date(col,'unixepoch')` across books and the session/stats tables
-- (F3.4) and forces per-call string parsing (`covers::get_last_modified_epoch`).
-- This converts all five to INTEGER unix-seconds; the in-DDL
-- `CAST(strftime('%s', col) AS INTEGER)` is the backfill (every prior writer
-- used `datetime('now')`/`CURRENT_TIMESTAMP`, both 'YYYY-MM-DD HH:MM:SS' UTC
-- that `strftime('%s', …)` parses exactly). Forward-only per rule 06.
--
-- `books.pubdate` is a genuinely partial *human* date and stays TEXT.
--
-- foreign_keys is ON (init_db sets it) and this runs inside the migrator's
-- implicit transaction, where `PRAGMA foreign_keys` is a no-op. A table
-- *recreate* of `books` would therefore fire ON DELETE CASCADE during the
-- implicit DROP-TABLE delete and wipe every child row (book_files, link
-- tables, book_identifiers, merge_log). So `books` is converted **in place**
-- via ADD/UPDATE/DROP/RENAME — no row delete, no cascade. The trade-off is
-- that `books.timestamp`/`last_modified` lose their column DEFAULT (SQLite
-- rejects a non-constant `ADD COLUMN` default on a populated table), so the
-- two book writers now set `strftime('%s','now')` explicitly. The four other
-- tables are child/standalone (nothing FK-references them), so they recreate
-- cleanly and keep the ideal `INTEGER NOT NULL DEFAULT (strftime('%s','now'))`.

-- books.timestamp + last_modified: TEXT -> INTEGER, in place.
-- DROP COLUMN refuses an indexed column, so drop the four dependent indexes
-- first and recreate them against the converted columns afterward.
DROP INDEX idx_books_timestamp;
DROP INDEX idx_books_last_modified;
DROP INDEX idx_books_library_timestamp;
DROP INDEX idx_books_library_last_modified;

ALTER TABLE books ADD COLUMN timestamp_epoch     INTEGER;
ALTER TABLE books ADD COLUMN last_modified_epoch INTEGER;
UPDATE books SET
    timestamp_epoch     = CAST(strftime('%s', timestamp)     AS INTEGER),
    last_modified_epoch = CAST(strftime('%s', last_modified) AS INTEGER);
ALTER TABLE books DROP COLUMN timestamp;
ALTER TABLE books DROP COLUMN last_modified;
ALTER TABLE books RENAME COLUMN timestamp_epoch     TO timestamp;
ALTER TABLE books RENAME COLUMN last_modified_epoch TO last_modified;

CREATE INDEX idx_books_last_modified ON books(last_modified);
CREATE INDEX idx_books_timestamp     ON books(timestamp);
CREATE INDEX idx_books_library_last_modified ON books (library_id, last_modified, id);
CREATE INDEX idx_books_library_timestamp     ON books (library_id, timestamp, id);

-- metadata_overrides.updated_at: TEXT -> INTEGER (recreate; nothing FK-refs it).
CREATE TABLE metadata_overrides_new (
    book_uuid          TEXT    NOT NULL PRIMARY KEY,
    overrides          TEXT    NOT NULL DEFAULT '{}',
    has_cover_override INTEGER NOT NULL DEFAULT 0,
    updated_by         INTEGER REFERENCES users(id) ON DELETE SET NULL,
    updated_at         INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
INSERT INTO metadata_overrides_new
    SELECT book_uuid, overrides, has_cover_override, updated_by,
           CAST(strftime('%s', updated_at) AS INTEGER)
      FROM metadata_overrides;
DROP TABLE metadata_overrides;
ALTER TABLE metadata_overrides_new RENAME TO metadata_overrides;
CREATE INDEX idx_metadata_overrides_updated ON metadata_overrides(updated_at);

-- author_photos.fetched_at: TEXT -> INTEGER (recreate; preserve both CHECKs).
CREATE TABLE author_photos_new (
    author_id   INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE,
    source      TEXT NOT NULL CHECK (source IN ('manual', 'openlibrary', 'letter')),
    url         TEXT,
    bytes       BLOB,
    mime        TEXT,
    fetched_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    CHECK (
        (source =  'letter' AND bytes IS NULL     AND mime IS NULL)
     OR (source <> 'letter' AND bytes IS NOT NULL AND mime IS NOT NULL)
    )
);
INSERT INTO author_photos_new
    SELECT author_id, source, url, bytes, mime,
           CAST(strftime('%s', fetched_at) AS INTEGER)
      FROM author_photos;
DROP TABLE author_photos;
ALTER TABLE author_photos_new RENAME TO author_photos;

-- merge_log.merged_at + undone_at: TEXT -> INTEGER (recreate; child of books).
CREATE TABLE merge_log_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    target_book_id  INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    source_uuid     TEXT    NOT NULL,
    source_metadata TEXT    NOT NULL,
    merged_by       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    merged_at       INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    undone_at       INTEGER
);
INSERT INTO merge_log_new
    SELECT id, target_book_id, source_uuid, source_metadata, merged_by,
           CAST(strftime('%s', merged_at) AS INTEGER),
           CASE WHEN undone_at IS NULL THEN NULL
                ELSE CAST(strftime('%s', undone_at) AS INTEGER) END
      FROM merge_log;
DROP TABLE merge_log;
ALTER TABLE merge_log_new RENAME TO merge_log;
CREATE INDEX idx_merge_log_target ON merge_log(target_book_id);

-- ignored_authors.ignored_at: DATETIME/CURRENT_TIMESTAMP -> INTEGER (recreate).
CREATE TABLE ignored_authors_new (
    name       TEXT    NOT NULL PRIMARY KEY COLLATE NOCASE,
    ignored_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
INSERT INTO ignored_authors_new
    SELECT name, CAST(strftime('%s', ignored_at) AS INTEGER) FROM ignored_authors;
DROP TABLE ignored_authors;
ALTER TABLE ignored_authors_new RENAME TO ignored_authors;
