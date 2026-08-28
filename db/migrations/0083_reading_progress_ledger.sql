-- Forward-progress ledger: what "pages read" is actually measured from.
--
-- The Est. pages tile summed the *length of every book finished in the
-- window*. Reading 90% of ten books reported nothing; flipping one long book
-- to finished dumped its whole length into whichever window the flip landed
-- in. Neither is pages read.
--
-- Nothing already stored can be differenced to fix that. `reading_progress`
-- holds one mutable upserted row per `(user, book, format)` whose
-- `progress_percent` is the *current* position only — the previous position is
-- overwritten, never kept — and the session tables record seconds with no
-- position at all. So the fix needs storage, and these two tables are it.
--
-- Two candidate shapes were on the table (issue #2139). The rejected one was a
-- start/end percent span on each session row: it keeps window attribution on
-- the rows the rest of stats already windows on, but it can only ever be as
-- good as the clients' session reporting — the web client posts sessions
-- best-effort on unmount, iOS's reader posts a CFI and no percent at all, and
-- every one of the five reporting surfaces (web reader, web comic reader, iOS
-- reader, iOS comic reader, iOS player) would have to learn to carry a
-- position before a single page could be counted. The shape chosen here
-- derives instead from the *position writes*, which every reading surface
-- already makes on every turn of a page, are queued through both offline
-- outboxes, and are format-correct by construction. No client changes; the web
-- and iOS tiles therefore agree on day one rather than after a client rollout.
--
-- `reading_progress_marks` is the last percent **observed** for a
-- `(user, book, format)`, and it is deliberately not the same value as
-- `reading_progress.progress_percent`: an epub write that carries only a CFI
-- (iOS) nulls the stored percent, which is then re-derived off the request path
-- by `spawn_epub_percent_derivation`. Reading the live row as "the previous
-- position" would therefore see NULL half the time and lose the gain. The mark
-- moves only when a percent is actually observed, from either path.
--
-- `reading_progress_daily` accrues `max(0, observed - mark)` into the UTC day
-- of the observation. Percent, not pages: the length ladder in
-- `db::stats::pages` is resolved at query time, so a book that later gains a
-- real `print_pages` override retroactively corrects its own history instead of
-- leaving a fossilised page figure behind. Backward moves accrue nothing (a
-- re-read forward counts again, which is what "pages read" means), and a book
-- whose first observation is its only one accrues nothing at all — the first
-- percent seen is a *baseline*, not a gain, so a device syncing a book it is
-- already 60% through does not report 60% of it as just-read.
--
-- The cutover is the direct consequence: reading before this migration ran left
-- no position trail to difference, so it cannot be reconstructed. The epoch is
-- recorded here rather than inferred, so the surfaces can state the date
-- outright instead of presenting an unexplained discontinuity.

CREATE TABLE reading_progress_marks (
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid TEXT    NOT NULL,
    format    TEXT    NOT NULL CHECK (format IN ('epub', 'audio')),
    percent   INTEGER NOT NULL CHECK (percent BETWEEN 0 AND 100),
    marked_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (user_id, book_uuid, format)
);

CREATE TABLE reading_progress_daily (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid      TEXT    NOT NULL,
    format         TEXT    NOT NULL CHECK (format IN ('epub', 'audio')),
    -- UTC `YYYY-MM-DD`, the same bucketing the heatmap uses.
    day            TEXT    NOT NULL,
    percent_gained INTEGER NOT NULL CHECK (percent_gained >= 0),
    updated_at     INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (user_id, book_uuid, day, format)
);

-- The primary key leads with `user_id` but then `book_uuid`, so a windowed
-- scan (`user_id = ? AND day >= ?`) can't use it past its first column.
CREATE INDEX idx_reading_progress_daily_user_day
    ON reading_progress_daily(user_id, day);

-- Both tables soft-reference `books.uuid` like every other durable user-data
-- table (rule 06): a reindex that ghosts a book must not delete the reading it
-- records. The stats queries join `books` themselves and drop what no longer
-- resolves.
CREATE INDEX idx_reading_progress_daily_book_uuid
    ON reading_progress_daily(book_uuid);

-- The cutover date, in the `settings` KV that already holds server-wide
-- internal values (`secret_key`). `INSERT OR IGNORE` so a re-run can never
-- move an epoch that has already been published to a UI.
INSERT OR IGNORE INTO settings (key, value)
VALUES ('pages_ledger_epoch', date('now'));
