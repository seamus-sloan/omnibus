-- Forward-FK indexes on link tables.
--
-- The m2m link tables in 0002 (`books_authors_link` etc.) declare
-- `PRIMARY KEY(book, <other>)`, which SQLite covers with an implicit
-- index on the composite. Lookups by the leading `book` column already
-- benefit from that PK index, so these explicit indexes are *not* a
-- functional win — they exist to make the planner's choice obvious
-- when reverse-lookup indexes (`idx_books_authors_author` etc.) are
-- the only named indexes on the table, and to give us a place to hang
-- a covering index later without touching the schema again.
--
-- `book_identifiers` is not a link table (its non-key column `value` is
-- per-book), but the same leading-column logic applies via its
-- `PRIMARY KEY(book_id, scheme)`. Indexed here for symmetry.
--
-- All indexes use `IF NOT EXISTS` so re-running the migration is a
-- no-op.

CREATE INDEX IF NOT EXISTS idx_books_authors_book     ON books_authors_link(book);
CREATE INDEX IF NOT EXISTS idx_books_series_book      ON books_series_link(book);
CREATE INDEX IF NOT EXISTS idx_books_tags_book        ON books_tags_link(book);
CREATE INDEX IF NOT EXISTS idx_books_publishers_book  ON books_publishers_link(book);
CREATE INDEX IF NOT EXISTS idx_books_languages_book   ON books_languages_link(book);
CREATE INDEX IF NOT EXISTS idx_book_identifiers_book  ON book_identifiers(book_id);
