-- F5.1: User metadata overrides.
--
-- Keyed by the stable UUID derived from library_path + filename via
-- stable_uuid() so overrides survive the DELETE/INSERT reindex cycle in
-- replace_books(). No FK to `books` — the table is orthogonal to the
-- indexer's atomic wipe-and-rewrite.
--
-- `overrides` holds a JSON object whose keys mirror EbookMetadata scalar
-- field names; values replace scanned values at read time. M2M fields
-- (creators, subjects) are stored as JSON arrays and replace entirely
-- (not merge/append).
--
-- `has_cover_override` flags that the user uploaded a replacement cover
-- stored at <covers_dir>/override-<uuid>.<ext>.

CREATE TABLE metadata_overrides (
    book_uuid          TEXT    NOT NULL PRIMARY KEY,
    overrides          TEXT    NOT NULL DEFAULT '{}',
    has_cover_override INTEGER NOT NULL DEFAULT 0,
    updated_by         INTEGER REFERENCES users(id) ON DELETE SET NULL,
    updated_at         TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_metadata_overrides_updated
    ON metadata_overrides(updated_at);
