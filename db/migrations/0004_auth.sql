-- F0.3: Authentication. Introduces users, devices, sessions, and a small
-- secrets table for the tower-sessions signing key. Matches the hardened
-- design in docs/roadmap/0-3-auth.md: Argon2id password hashes stored in
-- PHC string form, session tokens stored as SHA-256 of the raw token so a
-- DB leak cannot be replayed as live auth, explicit permission booleans
-- over a role bitmask (Calibre-inspection #9), and a shared session table
-- for both web cookies and mobile bearer tokens.
--
-- All time columns are Unix seconds (INTEGER) rather than TEXT so expiry
-- checks are a cheap integer compare in SQL. This matches the pattern
-- already used by `libraries.last_indexed` in migration 0002.

-- One row per human. is_admin + can_* are explicit booleans; F5.4 admin
-- panel edits them. Defaults for non-first users: is_admin=0, can_upload=0,
-- can_edit=0, can_download=1. The first user becomes admin via a race-free
-- INSERT inside BEGIN IMMEDIATE (see db::auth::create_user).
CREATE TABLE users (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    username            TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash       TEXT    NOT NULL,                                    -- PHC: $argon2id$v=19$m=...$salt$hash
    is_admin            INTEGER NOT NULL DEFAULT 0,
    can_upload          INTEGER NOT NULL DEFAULT 0,
    can_edit            INTEGER NOT NULL DEFAULT 0,
    can_download        INTEGER NOT NULL DEFAULT 1,
    failed_login_count  INTEGER NOT NULL DEFAULT 0,
    locked_until        INTEGER,                                             -- Unix seconds; NULL when not locked
    totp_secret         TEXT,                                                -- reserved for post-v1.0 MFA
    created_at          INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    password_changed_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

-- One row per registered client (browser profile, phone, tablet). The admin
-- panel lists these so a user can revoke a specific phone without nuking
-- every session. last_seen_ip is truncated to /24 (IPv4) or /48 (IPv6)
-- before storage so a DB leak does not expose full-resolution history.
CREATE TABLE devices (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT    NOT NULL,                                        -- user-visible: "Seamus's iPhone"
    client_kind     TEXT    NOT NULL,                                        -- 'web' | 'ios' | 'android'
    client_version  TEXT,
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_seen_at    INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_seen_ip    TEXT                                                     -- truncated /24 or /48
);

-- Unified session table for web cookies and mobile bearer tokens.
-- token_hash is SHA-256(raw_token) — the raw token is returned once at
-- login and never persisted. Rules out stateless JWT so revocation is a
-- single DELETE. Idle expiry is enforced by comparing last_used_at to an
-- application-level idle threshold; absolute expiry lives on expires_at.
CREATE TABLE sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash    BLOB    NOT NULL UNIQUE,                                    -- SHA-256(raw token), 32 bytes
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id     INTEGER          REFERENCES devices(id) ON DELETE SET NULL,
    kind          TEXT    NOT NULL CHECK(kind IN ('cookie', 'bearer')),       -- enforce enum at schema level
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_used_at  INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    expires_at    INTEGER NOT NULL,
    revoked_at    INTEGER
);

-- Small opaque k/v store for server-held secrets that must survive restart
-- without the operator managing env vars. v1.0 holds exactly one row: the
-- session signing key, auto-generated on first boot if neither
-- OMNIBUS_SESSION_KEY nor an existing row is present.
CREATE TABLE secrets (
    name       TEXT    PRIMARY KEY,
    value      BLOB    NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

-- Registration toggle. Starts enabled so the first user can register; the
-- application flips it to '0' inside the same transaction that creates the
-- first admin so a leaked URL cannot be used for un-authorized signup.
-- Idempotent because an operator on an existing DB may have seeded this
-- key already during a prior experimental build.
INSERT INTO settings (key, value) VALUES ('registration_enabled', '1')
ON CONFLICT(key) DO NOTHING;

-- `token_hash UNIQUE` already creates an implicit index; no explicit
-- idx_sessions_token_hash needed. The remaining indexes cover lookups
-- that are not served by a UNIQUE.
CREATE INDEX idx_sessions_user       ON sessions(user_id);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX idx_devices_user        ON devices(user_id);
