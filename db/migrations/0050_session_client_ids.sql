-- Client-generated identity for session reports.
--
-- A queued session report has no way to tell "the server never got this" from
-- "the server got this and the reply was lost" — a transport failure looks the
-- same either way, so the outbox keeps the op and replays it. Highlights,
-- bookmarks, and journals survive that replay because migration 0049 gave them
-- a client-minted handle to be idempotent on; sessions had none, so every lost
-- reply added a second row and inflated reading time for good.
--
-- Nullable: rows that predate this have no handle, and neither does a web
-- client, which posts once on unmount and never retries.

ALTER TABLE reading_sessions   ADD COLUMN client_id TEXT;
ALTER TABLE listening_sessions ADD COLUMN client_id TEXT;

-- Partial-unique so a replayed report collapses onto the row it already wrote,
-- while the NULLs left by pre-existing rows and by web clients stay
-- unconstrained. Scoped by user so one account's uuid can never collide with
-- another's.
CREATE UNIQUE INDEX reading_sessions_user_client_id_idx
    ON reading_sessions(user_id, client_id)
    WHERE client_id IS NOT NULL;

CREATE UNIQUE INDEX listening_sessions_user_client_id_idx
    ON listening_sessions(user_id, client_id)
    WHERE client_id IS NOT NULL;
