-- The forward-progress ledger stops deciding a day at write time.
--
-- 0083 accrued each gain into `date(observed_at, 'unixepoch')`. That makes the
-- day boundary UTC's, so a reader at UTC-4 watches their daily pages goal reset
-- at 8pm — and it bakes the decision into storage, where no later read can
-- revisit it. A day string is the one thing you cannot re-bucket.
--
-- Every other day-boundary figure in `db::stats` reads a timestamp and can
-- therefore be shifted to the reader's own calendar at query time. The ledger
-- has to be able to do the same, so the bucket key becomes a **quarter-hour
-- slot** — `observed_at / 900` — and the day is resolved on the way out.
--
-- Quarter-hour, not hourly: UTC+05:30 (India), UTC+05:45 (Nepal) and UTC-03:30
-- (Newfoundland) are real zones an hourly bucket cannot represent, and every
-- modern IANA offset is a whole number of quarter-hours. The row count stays
-- bounded — at most 96 per (reader, book, format, day), and in practice a
-- handful, since a reader covers ground in a few sittings rather than in 96.
-- Keying on the raw second instead would put one row on every turn of a page.
--
-- `reading_progress_daily` is deliberately left in place and frozen rather than
-- migrated. Its rows kept only a day string, so their true instant is unknown;
-- assigning them one — their day's UTC midnight, say — would let the read path
-- shift them into a *different* day and silently re-date history that was never
-- re-datable. The stats queries union the two generations instead: a legacy row
-- contributes its stored day verbatim, a slot row contributes a computed local
-- day. The old table stops being written and decays as its days age out of
-- every window.
--
-- Both tables soft-reference `books.uuid` like every other durable user-data
-- table (rule 06), so a reindex that ghosts a book does not delete the reading
-- it records, and both are on `merge::transaction`'s `RETARGET_TABLES`. Like
-- its predecessor this one is **summed** on a merge collision rather than
-- resolved latest-wins: it holds a counter, and a reader who covered ground in
-- both editions in the same quarter-hour covered all of it.

CREATE TABLE reading_progress_slots (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid      TEXT    NOT NULL,
    format         TEXT    NOT NULL CHECK (format IN ('epub', 'audio')),
    -- floor(observed_at / 900) — the quarter-hour the gain was observed in.
    slot           INTEGER NOT NULL,
    percent_gained INTEGER NOT NULL CHECK (percent_gained >= 0),
    updated_at     INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (user_id, book_uuid, format, slot)
);

-- The primary key leads with `user_id` but then `book_uuid`, so a windowed scan
-- (`user_id = ? AND slot >= ?`) cannot use it past its first column — the same
-- omission 0083 fixed for the daily table.
CREATE INDEX idx_reading_progress_slots_user_slot
    ON reading_progress_slots(user_id, slot);

-- `merge::transaction` retargets by `book_uuid` alone, with no `user_id`
-- predicate, so the primary key is unusable for that probe and the merge would
-- full-scan without this.
CREATE INDEX idx_reading_progress_slots_book_uuid
    ON reading_progress_slots(book_uuid);
