-- Position writes carry the writing device's clock, so a queued offline write
-- can be replayed without clobbering a newer position from another device.
--
-- `updated_at` is stamped at arrival and stays that way — it is what the resume
-- feed orders by. This column is a separate, client-supplied ordering key that
-- `upsert_progress` compares before it overwrites. Backfilled from `updated_at`
-- so every existing row has a comparable value from the first write onwards.
ALTER TABLE reading_progress ADD COLUMN client_updated_at INTEGER;

UPDATE reading_progress SET client_updated_at = updated_at WHERE client_updated_at IS NULL;
