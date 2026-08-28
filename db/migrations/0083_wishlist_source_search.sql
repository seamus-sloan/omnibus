-- Widen `wishlist_entries.source` to admit 'search' (#2247).
--
-- The check-in flow has three front doors — a scanned barcode, an ISBN typed
-- at the keypad, and a title search — and every entry added through any of
-- them was recorded as 'scan', so the detail page labelled a title-searched
-- book "added from a scan". 'manual' now carries the typed-ISBN case and
-- 'search' the title-search one.
--
-- SQLite can't ALTER a CHECK in place, so this is a table rebuild (the
-- pattern migration 0047 established). `wishlist_entries` is referenced by no
-- other table, so nothing cascades: the only FK is its own outbound one to
-- `users`, which the new table repeats.

CREATE TABLE wishlist_entries_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid  TEXT    NOT NULL,                             -- soft-ref to books.uuid
    added_at   INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    source     TEXT    NOT NULL CHECK (source IN ('scan','detail','manual','search'))
);
INSERT INTO wishlist_entries_new SELECT * FROM wishlist_entries;

DROP TABLE wishlist_entries;
ALTER TABLE wishlist_entries_new RENAME TO wishlist_entries;

-- Recreate 0045's indexes, which the drop took with the old table.
CREATE UNIQUE INDEX idx_wishlist_user_book ON wishlist_entries(user_id, book_uuid);
CREATE INDEX idx_wishlist_book ON wishlist_entries(book_uuid);
