-- Genre vocabulary table, mirroring `tags` from 0002.
--
-- Deliberately *no* `books_genres_link` companion. Every other taxonomy has
-- a link table because the scanner populates it from the file: `tags` comes
-- from OPF `<dc:subject>`, series from `belongs-to-collection`, and so on.
-- Nothing Omnibus parses — EPUB, CBZ's ComicInfo.xml, M4B atoms — carries a
-- genre, so a book's genres are sourced entirely from
-- `metadata_overrides.overrides -> '$.genres'`, exactly the way an
-- *overridden* subject list already is (see `materialize_tag_rows`).
--
-- That also makes a link table actively unsafe here: the indexer's Changed
-- bucket wipes a book's link rows and re-inserts them from the re-parsed
-- file, so genres kept in one would be erased on the next reindex with
-- nothing to restore them from. Overrides are keyed on the durable
-- `book_uuid` and survive it.
--
-- This table therefore holds the *vocabulary* only — one row per distinct
-- genre name, materialized on override write so the genre cloud (and the
-- chip-editor autocomplete it feeds) can offer a genre the moment a user
-- coins it. `delete_orphan_genres` reaps rows no live book's override still
-- names.
CREATE TABLE IF NOT EXISTS genres (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
