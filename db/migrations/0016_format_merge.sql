-- F5.10 format merge + index-time auto-attach.
--
-- `title_norm` / `author_norm` are the conservative match key for
-- attaching a newly indexed file to an existing book in another format
-- (exact match after casefold/punctuation/diacritics normalization —
-- computed in Rust, see db/src/normalize.rs). The sync writers store
-- NULL when nothing survives normalization; the one-time boot backfill
-- for pre-migration rows stores '' instead so its `title_norm IS NULL`
-- guard stays idempotent. Neither value can ever match — the attach
-- lookup only binds non-empty keys (and NULL never equals anything).
ALTER TABLE books ADD COLUMN title_norm  TEXT;
ALTER TABLE books ADD COLUMN author_norm TEXT;
CREATE INDEX idx_books_norm ON books(title_norm, author_norm);

-- Location override for files that don't live under their book's own
-- library/path — a cross-format attachment (or a manually merged file)
-- keeps pointing at its real on-disk home. NULL = inherit the book's
-- `libraries.path` / `books.path` exactly as before; the read paths
-- (`book_file_path`, `hls::resolve_audiobook`) COALESCE.
ALTER TABLE book_files ADD COLUMN library_path TEXT;
ALTER TABLE book_files ADD COLUMN path         TEXT;

-- Reindex protection: uuid -> surviving book. A scanned file whose
-- stable_uuid appears here belongs to (was merged into / attached to)
-- `book_id` instead of being inserted as its own books row.
-- `library_path` is the scanned root of the file behind the uuid (the
-- uuid embeds it, but not reversibly) so a reindex only diffs against
-- its own library's merged rows; the target book may live in a
-- *different* library. `format` identifies WHICH book_files row this
-- uuid's on-disk file backs; there is no FK to book_files because the
-- sync Changed path deletes and re-inserts those rows.
--
-- ON DELETE CASCADE: when the target book is deleted its merged uuids
-- vanish too, so a still-on-disk attached file reclassifies as New on
-- its next scan and re-attaches or recreates — self-healing.
CREATE TABLE merged_uuids (
    uuid         TEXT PRIMARY KEY,
    book_id      INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    format       TEXT    NOT NULL COLLATE NOCASE,
    library_path TEXT    NOT NULL
);
CREATE INDEX idx_merged_uuids_book ON merged_uuids(book_id);

-- Audit log for manual merges, with a JSON snapshot of the absorbed
-- book for undo. Cascades with the target: undo is impossible without
-- the surviving book anyway.
CREATE TABLE merge_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    target_book_id  INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    source_uuid     TEXT    NOT NULL,
    source_metadata TEXT    NOT NULL,
    merged_by       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    merged_at       TEXT    NOT NULL DEFAULT (datetime('now')),
    undone_at       TEXT
);
CREATE INDEX idx_merge_log_target ON merge_log(target_book_id);
