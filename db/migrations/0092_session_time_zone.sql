-- Capture-time IANA zone name on session rows, beside the offset 0080 added.
--
-- The offset answers "what did the clock say"; it cannot answer "where was I".
-- `-420` is Los Angeles in summer, Phoenix all year, and Denver in winter, so
-- the three are indistinguishable once recorded. That is fine for the
-- time-of-day and day-of-week strips, which only ever need the wall clock —
-- and those stay on the offset, which is DST-correct by construction because
-- the device reported the offset actually in effect at that moment.
--
-- What the offset cannot do is answer a question about a *different* instant.
-- The stats read path falls back to a reader's most recent session offset when
-- the requesting client reports none, and that answer goes stale across a DST
-- transition: read in Los Angeles in August, come back in January, and the
-- stored `-420` is an hour off the `-480` in force. A zone name resolves
-- correctly at any instant; an offset only at the one it was captured at.
--
-- Stored now, consumed later. Using it server-side means a tz database
-- (chrono-tz / jiff) and moving that arithmetic out of SQL into Rust, which is
-- not worth taking on for an hour of skew on a degraded path. But a zone name
-- cannot be backfilled from an offset, so it has to be captured while the rows
-- are being written or the option is gone for good.
--
-- Nullable, and NOT backfilled, for the same reason 0080 left its offset null:
-- a row that predates the column records nothing about where the reader was,
-- and deriving one from the offset would invent a fact rather than record one.

ALTER TABLE reading_sessions   ADD COLUMN time_zone TEXT;
ALTER TABLE listening_sessions ADD COLUMN time_zone TEXT;
