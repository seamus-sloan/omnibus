-- Kobo position round-trip (F4.1, #925): a percent + an opaque device location.
--
-- A Kobo reports its position as `CurrentBookmark` — a whole-book
-- `ProgressPercent` plus a `KoboSpan` location (`{Source, Type, Value}`, where
-- Value is `kobo.N.M` char-offsets into the `<span class="koboSpan">` wrappers
-- kepubify injects). That location is **not** an EPUB CFI: writing it into
-- `epub_cfi` would hand the web reader a string it cannot interpret, so it gets
-- its own opaque column and the two never mix.
--
-- The percent is the cross-surface half — it means the same thing on every
-- client — but today an epub row cannot exist without a CFI: the row CHECK
-- requires `epub_cfi IS NOT NULL` for format='epub'. A Kobo has no CFI to give,
-- so that CHECK is relaxed to "a position of *some* kind": a CFI, a percent, or
-- both. The audio arm is unchanged.
--
-- SQLite cannot alter a CHECK in place (ALTER TABLE does RENAME / ADD COLUMN /
-- DROP COLUMN only), so this is a full table rebuild following 0027's pattern:
-- CREATE _new, INSERT SELECT, DROP, RENAME, recreate every index by hand.

CREATE TABLE reading_progress_new (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id                  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid                TEXT    NOT NULL,
    format                   TEXT    NOT NULL CHECK (format IN ('epub', 'audio')),
    epub_cfi                 TEXT,
    audio_position_seconds   REAL,
    updated_at               INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    client_updated_at        INTEGER,
    -- Carried through from 0057_reading_progress_book_file: which audio file
    -- the position was taken in. A rebuild that omitted it would silently drop
    -- the column and every value in it.
    book_file_id             INTEGER,
    -- Whole-book percent, 0..=100. Nullable: only surfaces that report one set
    -- it, and an existing CFI-only row keeps NULL until the next write.
    progress_percent         INTEGER CHECK (progress_percent IS NULL
                                            OR progress_percent BETWEEN 0 AND 100),
    -- The device's `CurrentBookmark.Location` object, stored verbatim as JSON.
    -- Deliberately opaque: it is echoed back to the same device for exact
    -- resume and is never parsed, rewritten, or shown to another surface.
    kobo_location            TEXT,
    -- Both arms name every column they exclude, so a caller that bypasses the
    -- API validators still can't persist a cross-format row.
    CHECK (
        (format = 'epub'  AND (epub_cfi IS NOT NULL OR progress_percent IS NOT NULL)
                          AND audio_position_seconds IS NULL)
     OR (format = 'audio' AND audio_position_seconds IS NOT NULL
                          AND epub_cfi IS NULL
                          AND progress_percent IS NULL
                          AND kobo_location IS NULL)
    ),
    UNIQUE (user_id, book_uuid, format)
);
INSERT INTO reading_progress_new
    (id, user_id, book_uuid, format, epub_cfi, audio_position_seconds,
     updated_at, client_updated_at, book_file_id)
SELECT id, user_id, book_uuid, format, epub_cfi, audio_position_seconds,
       updated_at, client_updated_at, book_file_id
  FROM reading_progress;
DROP TABLE reading_progress;
ALTER TABLE reading_progress_new RENAME TO reading_progress;

-- Both indexes are load-bearing and asserted by tests: the first by
-- `db/src/progress/tests.rs`, the second by an EXPLAIN QUERY PLAN check in
-- `db/src/missing_files/tests.rs` (the GC victim query must SEARCH, not SCAN).
-- `reading_progress_user_book_idx` stays dropped (0021 removed it as redundant
-- with the UNIQUE auto-index; `db/src/pool/tests.rs` asserts its absence).
CREATE INDEX idx_reading_progress_user_updated ON reading_progress(user_id, updated_at);
CREATE INDEX idx_reading_progress_book_uuid    ON reading_progress(book_uuid);
