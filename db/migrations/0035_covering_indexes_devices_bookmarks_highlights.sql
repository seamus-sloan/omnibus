-- Covering indexes for three filter-then-sort read paths.
--
-- Each query seeks a leading equality column and orders by a trailing
-- column, but the pre-existing indexes cover only the filter — so SQLite
-- matches the rows then does an in-memory temp-B-tree sort. Extending each
-- index to include the ORDER BY column lets the planner walk the index in
-- output order and drop the sort entirely.
--
--   * devices    — list_devices_for_user (db/src/auth/device.rs):
--       WHERE user_id = ? ORDER BY last_seen_at DESC
--       had only idx_devices_user (user_id) from 0004.
--   * bookmarks  — list_bookmarks (db/src/bookmarks/mod.rs):
--       WHERE user_id = ? AND book_uuid = ? ORDER BY created_at ASC
--       had only bookmarks_user_book_idx (user_id, book_uuid) from 0027.
--   * highlights — list_highlights (db/src/highlights/mod.rs):
--       WHERE h.user_id = ? AND h.book_uuid = ? ORDER BY h.created_at ASC
--       had only idx_highlights_user_book (user_id, book_uuid) from 0027.
--
-- The row counts are small in typical usage, so this is a forward-looking
-- cleanup rather than a hot-path fix. Forward-only, pure secondary indexes:
-- populated on CREATE with no backfill and no table recreate, so a fresh DB
-- built from 0001..0035 and an upgraded DB converge on the same schema.
-- `IF NOT EXISTS` keeps each CREATE idempotent against a hand-made index of
-- the same name. The now partially-redundant filter-only indexes are left in
-- place deliberately — dropping them is a separate change.
CREATE INDEX IF NOT EXISTS idx_devices_user_last_seen
    ON devices(user_id, last_seen_at DESC);
CREATE INDEX IF NOT EXISTS idx_bookmarks_user_book_created
    ON bookmarks(user_id, book_uuid, created_at);
CREATE INDEX IF NOT EXISTS idx_highlights_user_book_created
    ON highlights(user_id, book_uuid, created_at);
