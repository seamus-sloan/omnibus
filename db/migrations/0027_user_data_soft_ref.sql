-- F1 — User data: `book_id` CASCADE FK → `book_uuid` soft reference.
--
-- The five user-data tables keyed on `books.id` with `ON DELETE CASCADE`, so a
-- reindex's Removed bucket (or any hard delete of a `books` row) silently took
-- every reading position, bookmark, session, and highlight with it. Now that
-- `books.uuid` is a durable, stable identity (F1's prerequisite F2, migration
-- 0026), key these tables on `book_uuid TEXT` with **no FK and no cascade** —
-- the `metadata_overrides` pattern (0007). A removed book's user data
-- *detaches* and survives as an orphan; it auto-relinks when the same uuid
-- reappears (which, post-F2, a ghost-retaining reindex guarantees).
--
-- SQLite can't drop a column's FK/cascade in place, so each table is a full
-- recreate: CREATE _new (book_uuid replacing book_id), backfill by joining the
-- old book_id to books.uuid, DROP, RENAME, recreate every index/CHECK/UNIQUE.
-- Rows whose book_id no longer joins a books row were already destroyed by the
-- old cascade and are simply not carried forward. The `device_id` FK on the
-- session tables is on *device*, not book — it is preserved verbatim.

-- reading_progress -----------------------------------------------------------
CREATE TABLE reading_progress_new (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id                  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid                TEXT    NOT NULL,
    format                   TEXT    NOT NULL CHECK (format IN ('epub', 'audio')),
    epub_cfi                 TEXT,
    audio_position_seconds   REAL,
    updated_at               INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    CHECK (
        (format = 'epub'  AND epub_cfi IS NOT NULL AND audio_position_seconds IS NULL)
     OR (format = 'audio' AND audio_position_seconds IS NOT NULL AND epub_cfi IS NULL)
    ),
    UNIQUE (user_id, book_uuid, format)
);
INSERT INTO reading_progress_new
    (id, user_id, book_uuid, format, epub_cfi, audio_position_seconds, updated_at)
SELECT rp.id, rp.user_id, b.uuid, rp.format, rp.epub_cfi,
       rp.audio_position_seconds, rp.updated_at
  FROM reading_progress rp
  JOIN books b ON b.id = rp.book_id;
DROP TABLE reading_progress;
ALTER TABLE reading_progress_new RENAME TO reading_progress;
-- `reading_progress_user_book_idx` was dropped by 0021 (redundant with the
-- UNIQUE auto-index); only the most-recent-write index from 0020 is restored.
CREATE INDEX idx_reading_progress_user_updated ON reading_progress(user_id, updated_at);

-- bookmarks ------------------------------------------------------------------
CREATE TABLE bookmarks_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid   TEXT    NOT NULL,
    position    TEXT    NOT NULL,
    title       TEXT,
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
INSERT INTO bookmarks_new (id, user_id, book_uuid, position, title, created_at)
SELECT bm.id, bm.user_id, b.uuid, bm.position, bm.title, bm.created_at
  FROM bookmarks bm
  JOIN books b ON b.id = bm.book_id;
DROP TABLE bookmarks;
ALTER TABLE bookmarks_new RENAME TO bookmarks;
CREATE INDEX bookmarks_user_book_idx ON bookmarks(user_id, book_uuid);

-- reading_sessions -----------------------------------------------------------
CREATE TABLE reading_sessions_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid       TEXT    NOT NULL,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER NOT NULL,
    seconds_read    INTEGER NOT NULL,
    device_id       INTEGER REFERENCES devices(id) ON DELETE SET NULL
);
INSERT INTO reading_sessions_new
    (id, user_id, book_uuid, started_at, ended_at, seconds_read, device_id)
SELECT rs.id, rs.user_id, b.uuid, rs.started_at, rs.ended_at, rs.seconds_read, rs.device_id
  FROM reading_sessions rs
  JOIN books b ON b.id = rs.book_id;
DROP TABLE reading_sessions;
ALTER TABLE reading_sessions_new RENAME TO reading_sessions;
CREATE INDEX reading_sessions_user_book_idx   ON reading_sessions(user_id, book_uuid);
CREATE INDEX idx_reading_sessions_user_started ON reading_sessions(user_id, started_at);

-- listening_sessions ---------------------------------------------------------
CREATE TABLE listening_sessions_new (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id            INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid          TEXT    NOT NULL,
    started_at         INTEGER NOT NULL,
    ended_at           INTEGER NOT NULL,
    seconds_listened   INTEGER NOT NULL,
    device_id          INTEGER REFERENCES devices(id) ON DELETE SET NULL
);
INSERT INTO listening_sessions_new
    (id, user_id, book_uuid, started_at, ended_at, seconds_listened, device_id)
SELECT ls.id, ls.user_id, b.uuid, ls.started_at, ls.ended_at, ls.seconds_listened, ls.device_id
  FROM listening_sessions ls
  JOIN books b ON b.id = ls.book_id;
DROP TABLE listening_sessions;
ALTER TABLE listening_sessions_new RENAME TO listening_sessions;
CREATE INDEX listening_sessions_user_book_idx   ON listening_sessions(user_id, book_uuid);
CREATE INDEX idx_listening_sessions_user_started ON listening_sessions(user_id, started_at);

-- highlights -----------------------------------------------------------------
CREATE TABLE highlights_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_uuid      TEXT    NOT NULL,
    epub_cfi_range TEXT    NOT NULL,
    color          TEXT    NOT NULL DEFAULT 'amber'
                   CHECK (color IN ('amber', 'green', 'blue', 'rose', 'violet')),
    note           TEXT,
    created_at     INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
INSERT INTO highlights_new
    (id, user_id, book_uuid, epub_cfi_range, color, note, created_at)
SELECT h.id, h.user_id, b.uuid, h.epub_cfi_range, h.color, h.note, h.created_at
  FROM highlights h
  JOIN books b ON b.id = h.book_id;
DROP TABLE highlights;
ALTER TABLE highlights_new RENAME TO highlights;
CREATE INDEX idx_highlights_user_book ON highlights(user_id, book_uuid);
