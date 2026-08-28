-- Capture-time UTC offset on session rows.
--
-- Every stats rollup buckets `started_at` in UTC. For a calendar-day heatmap
-- that skew is a rounding error; for an hour-of-day chart it is the whole
-- signal — a reader at UTC-7 who reads at 21:00 is otherwise reported reading
-- at 04:00. Recording the device's offset at the moment the session was
-- captured is the only answer that stays honest for a reader who travels: a
-- session read in Tokyo remains a Tokyo evening no matter where the account
-- is later used, and no client has to re-derive the bucketing (which is how
-- web, iOS, and a widget end up disagreeing about the same chart).
--
-- Minutes east of UTC, so UTC-7 is -420 and UTC+05:30 is 330 — every real
-- IANA offset is a whole number of minutes, and storing minutes rather than
-- hours keeps the half- and quarter-hour zones exact.
--
-- Nullable and deliberately NOT backfilled: rows that predate this carry no
-- record of where the reader was, and stamping them with UTC (or with any
-- later-observed offset) would invent a fact rather than record one. The
-- time-pattern rollups exclude them and report the excluded seconds instead;
-- see `db::stats::patterns`.

ALTER TABLE reading_sessions   ADD COLUMN utc_offset_minutes INTEGER;
ALTER TABLE listening_sessions ADD COLUMN utc_offset_minutes INTEGER;
