-- F2.1 Progress sync service.
--
-- One unified `reading_progress` table with a `format` discriminator column
-- backs both the epub reader and the (future) audio player. Last-write-wins
-- on `(user_id, book_id, format)` — the server is authoritative; conflicts
-- reconcile client-side.
--
-- `bookmarks` and the two `*_sessions` tables are designed-for-but-not-yet-
-- wired-to-UI in this cycle; their shape is fixed now so the audio player
-- (F2.3), bookmarks UI, and stats / year-in-review aggregation can ship
-- cheaply later.
CREATE TABLE reading_progress (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id                  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id                  INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    format                   TEXT    NOT NULL CHECK (format IN ('epub', 'audio')),
    epub_cfi                 TEXT,
    audio_position_seconds   REAL,
    updated_at               INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    -- A row carries exactly the position column its format implies; the
    -- other column must be NULL. Mirrors the source/bytes CHECK in
    -- `0008_author_photos.sql` so `db::progress::*` can trust row shape
    -- without re-validating per format.
    CHECK (
        (format = 'epub'  AND epub_cfi IS NOT NULL AND audio_position_seconds IS NULL)
     OR (format = 'audio' AND audio_position_seconds IS NOT NULL AND epub_cfi IS NULL)
    ),
    UNIQUE (user_id, book_id, format)
);
CREATE INDEX reading_progress_user_book_idx
    ON reading_progress(user_id, book_id);

-- Bookmarks live in their own table rather than a JSON blob on `users` so
-- adds/removes don't rewrite the whole user row (ABS recommendation #6) and
-- there's a real FK to books. Schema-only this cycle — no UI yet.
CREATE TABLE bookmarks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    position    TEXT    NOT NULL,
    title       TEXT,
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE INDEX bookmarks_user_book_idx ON bookmarks(user_id, book_id);

-- One row per reader open-to-close span. Feeds stats/year-in-review without
-- re-deriving from progress writes. `device_id` becomes NULL when its
-- device row is deleted so the historical session isn't lost.
CREATE TABLE reading_sessions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id         INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER NOT NULL,
    seconds_read    INTEGER NOT NULL,
    device_id       INTEGER REFERENCES devices(id) ON DELETE SET NULL
);
CREATE INDEX reading_sessions_user_book_idx
    ON reading_sessions(user_id, book_id);

-- Audio analog of `reading_sessions`. Mirrors ABS's PlaybackSession shape.
CREATE TABLE listening_sessions (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id            INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id            INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    started_at         INTEGER NOT NULL,
    ended_at           INTEGER NOT NULL,
    seconds_listened   INTEGER NOT NULL,
    device_id          INTEGER REFERENCES devices(id) ON DELETE SET NULL
);
CREATE INDEX listening_sessions_user_book_idx
    ON listening_sessions(user_id, book_id);
