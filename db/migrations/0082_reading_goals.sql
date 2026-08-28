-- Annual reading goals: one target per (user, calendar year, kind).
--
-- Per-year rather than a single mutable row so last year's target survives
-- into the recap this eventually feeds, and so raising 2027's goal can never
-- rewrite what 2026 was measured against.
--
-- Unlike `user_ratings` / `metadata_overrides`, a goal is *account
-- configuration*, not durable content about a book: it references no
-- `books.uuid`, and a deleted account has no goal worth keeping. So a hard FK
-- with ON DELETE CASCADE is correct here.
CREATE TABLE reading_goals (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Proleptic Gregorian calendar year the goal applies to, UTC — the same
    -- year `StatsRange::Year`'s window opens on.
    year       INTEGER NOT NULL,
    -- What is being counted. `books` is the only kind today; the column exists
    -- so a later pages/minutes goal is a new row rather than a new table.
    kind       TEXT    NOT NULL,
    -- Bounded in SQL as well as at the boundary: the progress ring divides by
    -- this, so a zero stored by any future write path would be a division the
    -- renderers have to defend against.
    target     INTEGER NOT NULL CHECK (target > 0),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    UNIQUE (user_id, year, kind)
);
