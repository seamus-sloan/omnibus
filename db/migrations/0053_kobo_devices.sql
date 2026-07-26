-- #923: per-DEVICE wireless-sync auth for Kobo. A user can register several
-- Kobos; each gets its own row carrying a re-displayable path token that the
-- reader pastes into its `api_endpoint` (`/kobo/<TOKEN>/v1/...`). Mirrors the
-- `devices` table shape (0004): owned by `user_id`, cascade-deleted with the
-- user, with `created_at` / `last_seen_at` bookkeeping.
--
-- Token storage — PLAINTEXT, deliberately (tradeoff): session tokens are
-- SHA-256 hash-only because they're never shown again. A Kobo token MUST be
-- re-displayable (AC2 — the Settings card shows the full endpoint URL so the
-- user can re-copy it), and there is no secure store on the Kobo to push a
-- freshly-minted secret to. So we store the raw token and look it up directly.
-- The credential is long-lived, per-device, per-user, and revocable
-- (regenerate rotates it in place; remove deletes the row) — the same posture
-- as an "API key shown in settings". UNIQUE gives the lookup its index.
--
-- `sync_cursor` is the opaque per-device snapshot cursor (AC3): kept on the
-- device row, INDEPENDENT of the auth token, so a token regenerate never
-- disturbs delta state. The full delta protocol is #926; this column just
-- reserves the hook so it isn't blocked.
--
-- `kobo_device_id` is the `x-kobo-deviceid` hardware id, learned and persisted
-- on the first tokened call, so later token-less Reading Services calls (#927)
-- can map back to the same device.
CREATE TABLE kobo_devices (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token           TEXT    NOT NULL UNIQUE,                                 -- re-displayable path credential (see comment)
    name            TEXT    NOT NULL,                                        -- user-visible label: "Kobo Clara"
    kobo_device_id  TEXT,                                                    -- x-kobo-deviceid, learned on first call (#927)
    sync_cursor     TEXT,                                                    -- opaque per-device delta cursor (#926), token-independent
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_seen_at    INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX idx_kobo_devices_user ON kobo_devices(user_id);
