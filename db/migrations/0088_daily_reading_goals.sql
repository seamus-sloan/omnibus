-- Daily reading goals: pages and minutes, alongside the annual books goal.
--
-- Migration 0082 anticipated further kinds — "the column exists so a later
-- pages/minutes goal is a new row rather than a new table" — but bound every
-- row to a calendar `year NOT NULL`. A daily goal is a *standing* target with
-- no year to file it under, so honouring that intent means the period has to
-- join the key rather than stay a fixed column.
--
-- `scope` names the period and `year` goes nullable, which SQLite can only do
-- by rebuilding the table: the create/copy/drop/rename shape migration 0027
-- established. Safe here for the reason 0038 spells out — `reading_goals` is a
-- child of `users` and nothing FK-references it, so the implicit DROP fires no
-- cascade into a child table and recreates cleanly.
--
-- The table-level UNIQUE is replaced by two **partial** unique indexes, and it
-- has to be: SQLite treats NULLs as distinct in a UNIQUE constraint, so
-- `UNIQUE (user_id, year, kind)` over a nullable `year` would let a reader
-- accumulate unlimited duplicate daily rows — the constraint would quietly
-- stop being a key at exactly the point this migration introduces.
--
-- `kind` stays unconstrained, as 0082 left it. The pairing of `scope` and
-- `year` is checked instead, because that one is structural: a daily row
-- carrying a year, or an annual row missing one, is a row no read path can
-- interpret.

CREATE TABLE reading_goals_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Which period the target is measured over: 'year' or 'day'.
    scope      TEXT    NOT NULL CHECK (scope IN ('year', 'day')),
    -- Proleptic Gregorian calendar year, UTC, for an annual goal; NULL for a
    -- daily one, which recurs rather than belonging to a year.
    year       INTEGER,
    kind       TEXT    NOT NULL,
    target     INTEGER NOT NULL CHECK (target > 0),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    CHECK ((scope = 'year' AND year IS NOT NULL)
        OR (scope = 'day'  AND year IS NULL))
);

INSERT INTO reading_goals_new (id, user_id, scope, year, kind, target, updated_at)
SELECT id, user_id, 'year', year, kind, target, updated_at FROM reading_goals;

DROP TABLE reading_goals;
ALTER TABLE reading_goals_new RENAME TO reading_goals;

-- One annual goal per (user, year, kind) — what the old table-level UNIQUE
-- enforced, now scoped so the nullable `year` can't reach it.
CREATE UNIQUE INDEX reading_goals_year_idx
    ON reading_goals(user_id, year, kind) WHERE scope = 'year';

-- One standing daily goal per (user, kind), so a reader runs at most one pages
-- goal and one minutes goal at a time.
CREATE UNIQUE INDEX reading_goals_day_idx
    ON reading_goals(user_id, kind) WHERE scope = 'day';
