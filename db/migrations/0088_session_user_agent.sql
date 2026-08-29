-- Record the `User-Agent` the session was issued to, so the sessions listing
-- can say *which* client holds a token ("Firefox on macOS") rather than only
-- its transport ("cookie").
--
-- Nullable with no backfill: the header is observable only at login, so rows
-- that predate this migration have nothing to recover. That heals on its own —
-- `prune_expired_sessions` drops a session 7 days after its last use, so every
-- surviving row carries a captured header within a week of deploy.
--
-- The raw header is stored (rather than a pre-parsed label) so the label stays
-- derived: improving the parser re-labels rows already on disk instead of
-- leaving a second, frozen copy of the same fact.

ALTER TABLE sessions ADD COLUMN user_agent TEXT;
