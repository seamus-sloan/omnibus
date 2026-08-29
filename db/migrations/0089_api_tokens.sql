-- Long-lived API tokens: deliberately-created bearer credentials with no
-- idle or absolute expiry — revocation is the whole lifecycle. Same at-rest
-- discipline as sessions: only SHA-256(raw token) is stored, never the raw.
-- `last_used_at` is NULL until the token first authenticates a request, so
-- the management UI can show "never used" honestly.
CREATE TABLE api_tokens (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash    BLOB    NOT NULL UNIQUE,                                    -- SHA-256(raw token), 32 bytes
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_used_at  INTEGER,
    revoked_at    INTEGER
);

CREATE INDEX idx_api_tokens_user ON api_tokens(user_id);
