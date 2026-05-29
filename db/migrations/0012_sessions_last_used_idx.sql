-- Make prune_expired_sessions index-driven instead of a full table scan.
--
-- The prune is a single DELETE with three OR'd predicates:
--   revoked_at IS NOT NULL  OR  expires_at <= ?  OR  last_used_at < ?
-- SQLite's OR-by-union optimization only kicks in when EVERY branch can be
-- served by an index; a single unindexed branch forces a full scan of the
-- whole table. 0004 already indexes expires_at, so we add the two missing
-- branches:
--   * last_used_at      -> the idle-expiry term (the gap #248 was filed for)
--   * revoked_at (partial, non-null only) -> the revoked term; partial so the
--     index only holds the handful of revoked rows rather than every session.
-- With all three branches indexed the planner switches from "SCAN sessions"
-- to a multi-index OR over the matching rows.
CREATE INDEX IF NOT EXISTS idx_sessions_last_used_at
  ON sessions(last_used_at);

CREATE INDEX IF NOT EXISTS idx_sessions_revoked_at
  ON sessions(revoked_at) WHERE revoked_at IS NOT NULL;
