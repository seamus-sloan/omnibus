-- Per-user half-star ratings (F3.2). User-generated data, so it soft-references
-- the durable `books.uuid` (no FK, no cascade) — a pruned/ghosted book detaches
-- its rating rather than deleting it, and a returning file at the same scan_key
-- re-links automatically because the uuid is preserved. The F10 missing-files GC
-- treats a book any rating references as never-purgeable (see missing_files.rs).
CREATE TABLE user_ratings (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid  TEXT    NOT NULL,                      -- soft ref: no FK, no CASCADE
    -- Rating in half-star units (1 = 0.5★ … 10 = 5.0★) so the UI can offer
    -- half-star granularity while the column stays an exact integer.
    half_stars INTEGER NOT NULL CHECK (half_stars BETWEEN 1 AND 10),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    UNIQUE (user_id, book_uuid)
);

-- `UNIQUE(user_id, book_uuid)` already indexes the per-user read path. The
-- missing-files GC probes `user_ratings` by `book_uuid` alone, so index that
-- column to serve the `NOT EXISTS (… WHERE book_uuid = ?)` guard.
CREATE INDEX idx_user_ratings_book ON user_ratings(book_uuid);
