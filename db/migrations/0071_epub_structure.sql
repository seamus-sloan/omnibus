-- Derived, regenerable per-file EPUB structure for cross-format position
-- mapping (mirrors 0015_file_chapters): per-spine-item visible-text stats
-- plus the table of contents resolved against the spine.
--
-- Both tables cascade from `book_files` deliberately — this is derived
-- data, not user data: the Changed sync path deletes and re-inserts the
-- `book_files` row, so a changed file's stale structure clears itself and
-- the post-scan backfill re-extracts it.
--
-- `epub_spine_stats` doubles as the extraction marker: it always gains
-- rows for a successfully opened EPUB (a spine is never empty), while
-- `ebook_chapters` may be legitimately empty (no NCX and no nav.xhtml).
-- The backfill's NOT EXISTS predicate keys on the stats table for that
-- reason.

CREATE TABLE epub_spine_stats (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    book_file_id  INTEGER NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    spine_index   INTEGER NOT NULL,
    -- OPF-root-relative href of the spine document, '/'-normalized.
    href          TEXT    NOT NULL DEFAULT '',
    -- Visible characters in this document, in the kobo_position walk's
    -- normalization (whitespace runs collapsed, entities resolved).
    visible_chars INTEGER NOT NULL DEFAULT 0,
    -- Sum of visible_chars over spine items strictly before this one, so
    -- percent-at-position is O(1) arithmetic over two rows.
    chars_before  INTEGER NOT NULL DEFAULT 0,
    UNIQUE(book_file_id, spine_index)
);
CREATE INDEX idx_epub_spine_stats_file ON epub_spine_stats(book_file_id);

CREATE TABLE ebook_chapters (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    book_file_id  INTEGER NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    ordinal       INTEGER NOT NULL,
    title         TEXT    NOT NULL DEFAULT '',
    -- The href the TOC entry pointed at (OPF-root-relative, fragment
    -- kept) — retained for a later fragment-precision upgrade.
    href          TEXT    NOT NULL DEFAULT '',
    spine_index   INTEGER NOT NULL DEFAULT 0,
    -- Cumulative visible chars at the chapter's spine-item start: the
    -- chapter's whole-book text offset, at spine granularity.
    start_chars   INTEGER NOT NULL DEFAULT 0,
    UNIQUE(book_file_id, ordinal)
);
CREATE INDEX idx_ebook_chapters_file ON ebook_chapters(book_file_id);
