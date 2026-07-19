-- Physical Check-In foundation: physical ownership + a physical wishlist.
--
-- `physical_copies` is library-wide (like digital `book_files`): a book can
-- hold multiple copies, each individually deletable ("I sold it"). The scanned
-- ISBN is stored per copy. `wishlist_entries` is per-user — the *physical*
-- wishlist, orthogonal to digital ownership (you can wishlist a book whose
-- EPUB you already have). Checking in a copy fulfills (deletes) every user's
-- wishlist entry for that book in the same transaction.
--
-- Both tables SOFT-reference `books.uuid` (no FK, no cascade) per the
-- durable-user-data rule: a reindex / scan-root repoint / format merge that
-- ghosts a book must not wipe ownership or wishlist rows. Resolve a uuid to a
-- live book through `resolve_book_id_by_uuid` (which honors `merged_uuids`).

CREATE TABLE physical_copies (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    book_uuid        TEXT    NOT NULL,                       -- soft-ref to books.uuid
    isbn             TEXT,                                   -- scanned ISBN; NULL for a manual add
    added_by_user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    checked_in_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    note             TEXT                                    -- free-text edition/publisher
);

-- Book detail lists a book's copies; check-in fulfillment sweeps by book.
CREATE INDEX idx_physical_copies_book ON physical_copies(book_uuid);

CREATE TABLE wishlist_entries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid  TEXT    NOT NULL,                             -- soft-ref to books.uuid
    added_at   INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    source     TEXT    NOT NULL CHECK (source IN ('scan','detail','manual'))
);

-- A book is on a user's wishlist at most once; re-wishlisting is idempotent.
CREATE UNIQUE INDEX idx_wishlist_user_book ON wishlist_entries(user_id, book_uuid);
-- Fulfillment deletes every user's entry for a checked-in book — index the sweep.
CREATE INDEX idx_wishlist_book ON wishlist_entries(book_uuid);
