-- Store the `Statistics` block a Kobo sends on `PUT library/{uuid}/state`
-- (#1653) so sync-out can echo it back. The device arbitrates the block by
-- timestamp like every other sub-object (#1652), so it must be echoed with
-- the DEVICE's clock: emitting zeroed counters stamped `now` would make the
-- server assert "your reading totals are zero" and the device would adopt it.
-- Hence a third column for the device's own `Statistics.LastModified` — the
-- values are useless without the clock that makes them safe to send.
--
-- `reading_progress` is the natural home: per (user, book, format), and Kobo
-- only ever writes `epub`. These are NOT fed into `db::stats` — cumulative
-- totals would double-count against the discrete `reading_sessions` rows the
-- `LeaveContent` analytics event already writes.
--
-- SQLite can't add a column to an existing CHECK, and the audio arm below
-- names every column it excludes so a caller bypassing the API validators
-- still can't persist a cross-format row. Keeping that guarantee means the
-- 0058 dance: CREATE _new, INSERT SELECT, DROP, RENAME, recreate every index.

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
    -- Deliberately opaque on the echo path: it rides back to the same device
    -- unchanged. `db::kobo_position` does parse it, but only to DERIVE the
    -- `epub_cfi` beside it — the blob itself is never rewritten from that.
    kobo_location            TEXT,
    -- The device's own `Statistics` counters, echoed back untouched. Negative
    -- values are rejected rather than clamped: the counters are only ever
    -- handed straight back, so a value we don't understand has no safe
    -- interpretation to clamp toward.
    kobo_spent_reading_minutes   INTEGER CHECK (kobo_spent_reading_minutes IS NULL
                                                OR kobo_spent_reading_minutes >= 0),
    kobo_remaining_time_minutes  INTEGER CHECK (kobo_remaining_time_minutes IS NULL
                                                OR kobo_remaining_time_minutes >= 0),
    -- The device's `Statistics.LastModified`, NOT server receipt time. Emitting
    -- a fresh stamp would win the device's newest-wins arbitration and overwrite
    -- its real totals with ours. NULL when the device sent counters but no
    -- parseable clock: sync-out then omits the stamp, which loses arbitration
    -- and leaves the device's own totals alone — the safe direction.
    kobo_statistics_updated_at   INTEGER,
    -- Both arms name every column they exclude, so a caller that bypasses the
    -- API validators still can't persist a cross-format row.
    CHECK (
        (format = 'epub'  AND (epub_cfi IS NOT NULL OR progress_percent IS NOT NULL)
                          AND audio_position_seconds IS NULL)
     OR (format = 'audio' AND audio_position_seconds IS NOT NULL
                          AND epub_cfi IS NULL
                          AND progress_percent IS NULL
                          AND kobo_location IS NULL
                          AND kobo_spent_reading_minutes IS NULL
                          AND kobo_remaining_time_minutes IS NULL
                          AND kobo_statistics_updated_at IS NULL)
    ),
    UNIQUE (user_id, book_uuid, format)
);
INSERT INTO reading_progress_new
    (id, user_id, book_uuid, format, epub_cfi, audio_position_seconds,
     updated_at, client_updated_at, book_file_id, progress_percent, kobo_location)
SELECT id, user_id, book_uuid, format, epub_cfi, audio_position_seconds,
       updated_at, client_updated_at, book_file_id, progress_percent, kobo_location
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
