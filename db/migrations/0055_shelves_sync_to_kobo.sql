-- Per-shelf opt-in for native Kobo wireless sync (F4.1, #924).
--
-- Wireless sync is never whole-library: a book reaches a device only by sitting
-- on a shelf its owner has explicitly marked. Defaulting to 0 means the flag is
-- off for every shelf that already exists, so enabling the feature on an
-- upgraded install syncs nothing until the user opts a shelf in.
--
-- A plain ADD COLUMN is safe here — no CHECK constraint changes, so none of the
-- table-rebuild dance that 0047 needed.

ALTER TABLE shelves ADD COLUMN sync_to_kobo INTEGER NOT NULL DEFAULT 0;

-- The sync query filters by (owner, opted-in) on every device request; the
-- partial index keeps that lookup off a full scan while staying tiny, since
-- opted-in shelves are a small minority of rows.
CREATE INDEX IF NOT EXISTS idx_shelves_sync_to_kobo
    ON shelves (owner_user_id)
    WHERE sync_to_kobo = 1;
