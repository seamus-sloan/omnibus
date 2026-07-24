-- Per-user read/unread state (F3.4 reading stats). Lets a user mark a book
-- read/unread directly from book detail without writing a journal entry.
--
-- User-generated data, so it soft-references the durable `books.uuid` (no FK,
-- no cascade) exactly like `user_ratings` (0031): a pruned/ghosted book detaches
-- its read state rather than deleting it, and a returning file at the same
-- scan_key re-links automatically because the uuid is preserved. The F10
-- missing-files GC treats a book any read-status row references as
-- never-purgeable (see missing_files.rs).
--
-- `status` is a three-state lifecycle: `unread` (never opened / cleared),
-- `reading` (in progress), `finished` (completed). Absence of a row is treated
-- as `unread` everywhere it's read (shelves, hero display). `finished_at` is
-- the unix-seconds moment the book most recently became `finished` (NULL for
-- the other two states) so the reading-stats aggregations can window completions
-- the same way the journal path windows `journal_entries.created_at`.
CREATE TABLE book_read_status (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid   TEXT    NOT NULL,                     -- soft ref: no FK, no CASCADE
    status      TEXT    NOT NULL CHECK (status IN ('unread', 'reading', 'finished')),
    updated_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    finished_at INTEGER,                              -- unix secs; NULL unless status = 'finished'
    UNIQUE (user_id, book_uuid)
);

-- `UNIQUE(user_id, book_uuid)` already indexes the per-user read path. The
-- missing-files GC and the smart-shelf `status` rule both probe by `book_uuid`
-- alone, so index that column to serve the `EXISTS (… WHERE book_uuid = ?)`
-- guards.
CREATE INDEX idx_book_read_status_book ON book_read_status(book_uuid);
