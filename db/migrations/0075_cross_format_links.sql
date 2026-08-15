-- Per-user cross-format sync links: syncing a dual-format book's text and
-- audio positions is OFF until the user confirms the alignment, and this
-- row is that confirmation. User data (not derived), so it soft-references
-- books.uuid per rule 06 and survives reindex; the audio snapshot pins the
-- file set that was confirmed, so any later change to the book's audio
-- files pauses mapping until the user re-confirms.
CREATE TABLE cross_format_links (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id              INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid            TEXT    NOT NULL,
    -- How multiple audio files relate: played end to end ('sequence') or
    -- each a complete recording ('narrations'). A single-file book is a
    -- degenerate 'sequence'.
    mode                 TEXT    NOT NULL CHECK (mode IN ('sequence', 'narrations')),
    -- narrations mode: the aligned primary audio file. Soft reference — a
    -- stale id pauses mapping rather than cascading anything.
    primary_book_file_id INTEGER,
    -- JSON [{"scan_key": …, "etag": …}, …] of the book's audio files at
    -- confirm time, sorted by scan_key. A mismatch against the current
    -- set means the link needs re-confirmation.
    audio_snapshot       TEXT    NOT NULL DEFAULT '[]',
    confirmed_at         INTEGER NOT NULL,
    UNIQUE (user_id, book_uuid),
    CHECK (mode != 'narrations' OR primary_book_file_id IS NOT NULL)
);
CREATE INDEX idx_cross_format_links_book ON cross_format_links(book_uuid);
