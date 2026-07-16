-- Per-user, per-audiobook playback speed. The durable book UUID is a soft
-- reference so preferences survive reindex/ghosting like other user data.
CREATE TABLE audiobook_playback_preferences (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid       TEXT    NOT NULL,
    playback_rate   REAL    NOT NULL CHECK (playback_rate BETWEEN 0.5 AND 3.0),
    updated_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    UNIQUE (user_id, book_uuid)
);

CREATE INDEX idx_audiobook_playback_preferences_book_uuid
    ON audiobook_playback_preferences(book_uuid);
