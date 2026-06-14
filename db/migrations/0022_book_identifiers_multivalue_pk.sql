-- F7: widen the identifier PK so two distinct values for one scheme both
-- survive. The 0002 PRIMARY KEY(book_id, scheme) let at most one value per
-- scheme exist, so a book carrying both an ISBN-10 and an ISBN-13 (or print
-- vs ebook editions in one OPF) silently lost one to OR REPLACE. The wider
-- PRIMARY KEY(book_id, scheme, value) keeps every distinct tuple; the writer
-- switches to OR IGNORE (db/src/sync/books.rs) so exact-duplicate tuples are
-- deduped instead of erroring.
--
-- Forward-only table recreate: SQLite cannot ALTER a PRIMARY KEY in place.
-- Column definitions (FK + COLLATE NOCASE) are copied verbatim from 0002.
CREATE TABLE book_identifiers_new (
    book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    scheme  TEXT    NOT NULL COLLATE NOCASE,
    value   TEXT    NOT NULL COLLATE NOCASE,
    PRIMARY KEY(book_id, scheme, value)
);

-- The old PK guaranteed at most one row per (book_id, scheme), so every
-- existing row is already unique on the wider (book_id, scheme, value) key —
-- the copy can never violate the new PK and loses nothing. (Duplicates that
-- 0002 collapsed away are gone from the DB already; they reappear on the next
-- reindex now that the writer keeps them.)
INSERT INTO book_identifiers_new (book_id, scheme, value)
    SELECT book_id, scheme, value FROM book_identifiers;

DROP TABLE book_identifiers;
ALTER TABLE book_identifiers_new RENAME TO book_identifiers;

-- Recreate the secondary scheme/value lookup index (0002). It was attached to
-- the dropped table and is not covered by the PK's column order.
CREATE INDEX idx_book_identifiers_scheme ON book_identifiers(scheme, value);
