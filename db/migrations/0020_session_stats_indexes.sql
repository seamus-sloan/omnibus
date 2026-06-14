-- F3.4 stats: windowed-aggregate indexes for the session tables.
--
-- The stats feature groups a user's sessions by day over a date range
-- (`GROUP BY date(started_at)`, `SUM(seconds_read)`) filtered by user,
-- targeting < 250 ms cold on 10k events. The existing `(user_id, book_id)`
-- indexes from 0013 can't range-scan a time window, so each `/stats`
-- query would otherwise degrade to a per-user table scan filtered in
-- memory by date. `(user_id, started_at)` lets SQLite range-scan the
-- window directly.
--
-- `reading_progress` gets `(user_id, updated_at)` for most-recent-write
-- ordering (the "continue reading" rail). No `(user_id, ended_at)` index
-- is added — no current or roadmap query ranges or groups on `ended_at`
-- (duration is the precomputed `seconds_read`/`seconds_listened` column),
-- and the repo bans speculative indexes.
CREATE INDEX idx_reading_sessions_user_started   ON reading_sessions(user_id, started_at);
CREATE INDEX idx_listening_sessions_user_started ON listening_sessions(user_id, started_at);
CREATE INDEX idx_reading_progress_user_updated   ON reading_progress(user_id, updated_at);
