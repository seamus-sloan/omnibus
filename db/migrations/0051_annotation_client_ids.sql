-- Client-generated identity for highlights and bookmarks.
--
-- An offline client can create an annotation and then edit or delete it before
-- either op has reached the server. Replaying that pair against server-assigned
-- ids is incoherent: the create returns an id the queued edit/delete never saw,
-- so the follow-up addresses a row that does not exist and is dropped — leaving
-- a highlight the user deleted still on the page. A uuid minted at creation time
-- on the device gives every op a stable handle from the moment of the gesture.
--
-- Nullable: every row that predates this migration has no client id, and the
-- numeric id remains a valid handle for both those rows and any online client
-- that never sends one.

ALTER TABLE highlights      ADD COLUMN client_id TEXT;
ALTER TABLE bookmarks       ADD COLUMN client_id TEXT;
ALTER TABLE journal_entries ADD COLUMN client_id TEXT;

-- Partial-unique so a replayed create is idempotent rather than a duplicate,
-- while the NULLs left by pre-existing rows stay unconstrained. Scoped by user
-- so one account's uuid can never collide with another's.
CREATE UNIQUE INDEX highlights_user_client_id_idx
    ON highlights(user_id, client_id)
    WHERE client_id IS NOT NULL;

CREATE UNIQUE INDEX bookmarks_user_client_id_idx
    ON bookmarks(user_id, client_id)
    WHERE client_id IS NOT NULL;

CREATE UNIQUE INDEX journal_entries_user_client_id_idx
    ON journal_entries(user_id, client_id)
    WHERE client_id IS NOT NULL;
