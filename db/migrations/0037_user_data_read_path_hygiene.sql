-- Covering indexes for three per-user list queries that filter by a
-- leading column and sort by a trailing column. Without these, SQLite
-- must sort the matched rows in memory (`USE TEMP B-TREE FOR ORDER BY`).
--
-- - `devices`:    `list_devices_for_user` filters by `user_id`, sorts
--                 by `last_seen_at DESC`.
-- - `bookmarks`:  `list_bookmarks` filters by `(user_id, book_uuid)`,
--                 sorts by `created_at ASC`.
-- - `highlights`: `list_highlights` filters by `(user_id, book_uuid)`,
--                 sorts by `created_at ASC`.
--
-- The existing single-column indexes (`idx_devices_user`,
-- `bookmarks_user_book_idx`, `idx_highlights_user_book`) are left in
-- place — SQLite will pick the covering index for the ORDER BY case
-- and either for lookup-only queries.

CREATE INDEX IF NOT EXISTS idx_devices_user_last_seen
    ON devices(user_id, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_bookmarks_user_book_created
    ON bookmarks(user_id, book_uuid, created_at ASC);

CREATE INDEX IF NOT EXISTS idx_highlights_user_book_created
    ON highlights(user_id, book_uuid, created_at ASC);
