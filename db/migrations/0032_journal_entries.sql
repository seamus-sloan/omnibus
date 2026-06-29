-- Public per-book journal entries (F3.2). Every entry is visible to all
-- authenticated users (a shared reading log), with author attribution via
-- `user_id`; only the owner may edit or delete their own rows.
--
-- User-generated data, so it soft-references the durable `books.uuid` (no FK,
-- no cascade) — a pruned/ghosted book detaches its entries rather than deleting
-- them, and a returning file at the same scan_key re-links automatically because
-- the uuid is preserved. The F10 missing-files GC treats a book any journal
-- entry references as never-purgeable (see missing_files.rs).
CREATE TABLE journal_entries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid  TEXT    NOT NULL,                       -- soft ref: no FK, no CASCADE
    body_md    TEXT    NOT NULL,                        -- raw markdown source
    -- Optional reading progress (0..100%) captured at the time of the entry.
    progress   INTEGER CHECK (progress IS NULL OR (progress BETWEEN 0 AND 100)),
    created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

-- The book-detail feed lists every user's entries for one book, newest first,
-- so index `(book_uuid, created_at)`. The missing-files GC probes by
-- `book_uuid` alone, which this index also serves.
CREATE INDEX idx_journal_entries_book ON journal_entries(book_uuid, created_at);
-- A user's `ON DELETE CASCADE` removal walks rows by `user_id`.
CREATE INDEX idx_journal_entries_user ON journal_entries(user_id);
