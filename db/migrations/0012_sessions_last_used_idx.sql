-- Make prune_expired_sessions index-driven instead of a full table scan.
--
-- Even with all three columns indexed, SQLite still plans "SCAN sessions" for
-- the single OR'd DELETE (verified via EXPLAIN QUERY PLAN), so the prune issues
-- three separate single-predicate DELETEs, each served by its index. 0004 has
-- idx_sessions_expires_at; these add the idle (last_used_at) and revoked terms.
-- The revoked index is partial (non-null only) so it holds just revoked rows.
CREATE INDEX IF NOT EXISTS idx_sessions_last_used_at
  ON sessions(last_used_at);

CREATE INDEX IF NOT EXISTS idx_sessions_revoked_at
  ON sessions(revoked_at) WHERE revoked_at IS NOT NULL;
