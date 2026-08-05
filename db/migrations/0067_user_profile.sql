-- Per-user display profile: an optional presentation name and an avatar image.
--
-- `username` remains the immutable login/admin identity; `display_name` is what
-- other users see (attribution reads COALESCE(display_name, username)). The
-- wishlist shelf name is denormalized from that same expression, so
-- `set_display_name` renames it in the same transaction — migration 0054's
-- "usernames are immutable, so the name can't drift" no longer holds on its own.
ALTER TABLE users ADD COLUMN display_name TEXT;

-- One avatar per user, replaced in place on upload (mirrors author_photos,
-- minus the source/negative-cache machinery — there is no automatic resolution
-- here, only what the user uploaded). Bytes live in the DB so backups stay one
-- file and replace-on-upload is a plain upsert with no orphan sweep.
CREATE TABLE user_avatars (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    mime       TEXT    NOT NULL,
    bytes      BLOB    NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
