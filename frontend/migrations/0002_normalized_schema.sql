-- F0.1: Normalized schema. Renames the legacy 0001 tables with a `_legacy`
-- suffix so code can use the final table names immediately, then creates
-- the new normalized layout. Migration 0003 drops the renamed legacy tables
-- after the code has cut over; leaving them in a separate migration
-- preserves rollback-before-deploy reversibility.

-- Preserve legacy data for one migration window.
ALTER TABLE books              RENAME TO books_legacy;
ALTER TABLE book_covers        RENAME TO book_covers_legacy;
ALTER TABLE library_index_state RENAME TO library_index_state_legacy;

-- Configured scan directories. Replaces the two singleton settings keys
-- (those keys continue to populate `libraries` via auto-migration on startup
-- so the settings UI can stay unchanged for now).
CREATE TABLE libraries (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    path          TEXT    NOT NULL UNIQUE,
    display_name  TEXT    NOT NULL,
    last_indexed  INTEGER
);

-- Logical work. One row per book regardless of how many file formats exist.
CREATE TABLE books (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid           TEXT    NOT NULL UNIQUE,
    library_id     INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    path           TEXT    NOT NULL,
    title          TEXT    NOT NULL COLLATE NOCASE,
    sort           TEXT        COLLATE NOCASE,
    author_sort    TEXT        COLLATE NOCASE,
    series_index   REAL,
    pubdate        TEXT,
    timestamp      TEXT    NOT NULL DEFAULT (datetime('now')),
    last_modified  TEXT    NOT NULL DEFAULT (datetime('now')),
    has_cover      INTEGER NOT NULL DEFAULT 0,
    description    TEXT        COLLATE NOCASE,
    isbn           TEXT        COLLATE NOCASE
);

-- One row per physical file (EPUB, M4B, ...). filename is the stem without
-- extension; format holds the uppercase extension.
CREATE TABLE book_files (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    format      TEXT    NOT NULL COLLATE NOCASE,
    filename    TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL,
    mtime       TEXT    NOT NULL,
    UNIQUE(book_id, format)
);

-- Normalized taxonomy tables. Dedupe happens via UNIQUE(name) NOCASE.
CREATE TABLE authors    (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE COLLATE NOCASE, sort TEXT COLLATE NOCASE);
CREATE TABLE series     (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE COLLATE NOCASE, sort TEXT COLLATE NOCASE);
CREATE TABLE tags       (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
CREATE TABLE publishers (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
CREATE TABLE languages  (id INTEGER PRIMARY KEY AUTOINCREMENT, code TEXT NOT NULL UNIQUE COLLATE NOCASE);

-- m2m link tables. Compound PK; cascade deletes from books.
CREATE TABLE books_authors_link    (book INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, author    INTEGER NOT NULL REFERENCES authors(id),    position INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(book, author));
CREATE TABLE books_series_link     (book INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, series    INTEGER NOT NULL REFERENCES series(id),     PRIMARY KEY(book, series));
CREATE TABLE books_tags_link       (book INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, tag       INTEGER NOT NULL REFERENCES tags(id),       PRIMARY KEY(book, tag));
CREATE TABLE books_publishers_link (book INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, publisher INTEGER NOT NULL REFERENCES publishers(id), PRIMARY KEY(book, publisher));
CREATE TABLE books_languages_link  (book INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, language  INTEGER NOT NULL REFERENCES languages(id),  PRIMARY KEY(book, language));

-- Per-book identifier list, scheme-scoped so duplicates like two ISBNs
-- collapse. Not a link table because the value is book-specific.
CREATE TABLE book_identifiers (
    book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    scheme  TEXT    NOT NULL COLLATE NOCASE,
    value   TEXT    NOT NULL COLLATE NOCASE,
    PRIMARY KEY(book_id, scheme)
);

-- Indices: sort/search columns on books + every m2m reverse column.
CREATE INDEX idx_books_uuid              ON books(uuid);
CREATE INDEX idx_books_sort              ON books(sort);
CREATE INDEX idx_books_author_sort       ON books(author_sort);
CREATE INDEX idx_books_series_index      ON books(series_index);
CREATE INDEX idx_books_last_modified     ON books(last_modified);
CREATE INDEX idx_books_timestamp         ON books(timestamp);
CREATE INDEX idx_books_library_id        ON books(library_id);
CREATE INDEX idx_book_files_book_id      ON book_files(book_id);
CREATE INDEX idx_books_authors_author    ON books_authors_link(author);
CREATE INDEX idx_books_series_series     ON books_series_link(series);
CREATE INDEX idx_books_tags_tag          ON books_tags_link(tag);
CREATE INDEX idx_books_publishers_pub    ON books_publishers_link(publisher);
CREATE INDEX idx_books_languages_lang    ON books_languages_link(language);
CREATE INDEX idx_book_identifiers_scheme ON book_identifiers(scheme, value);
