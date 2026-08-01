-- Per-user opt-in for syncing web-created annotations down to Kobo devices.
--
-- Defaulting to 0 preserves the wireless channel's device-origin-only
-- behavior for every existing account: until a user opts in, no web-origin
-- row gains a derived KoboSpan anchor, so the serving queries (which filter
-- on `kobo_location IS NOT NULL`, unchanged) keep answering exactly what
-- they did. The toggle gates *materialization*, not serving — a row that
-- has already been converted keeps syncing even if the toggle is later
-- turned off, because the device already holds it and delete-by-omission
-- would wipe it there.
--
-- A plain ADD COLUMN is safe here — no CHECK constraint changes, so none of
-- the table-rebuild dance that 0059 needed.

ALTER TABLE users ADD COLUMN sync_annotations_to_kobo INTEGER NOT NULL DEFAULT 0;
