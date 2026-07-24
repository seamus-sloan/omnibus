-- Wishlist as a built-in system shelf (F Physical Check-In, #1187).
--
-- Every user gets one locked, public 'wishlist'-kind shelf whose membership is
-- derived from `wishlist_entries` (not `shelf_books`). Widening the `kind`
-- CHECK to admit 'wishlist' needs a table rebuild — SQLite can't ALTER a CHECK
-- in place.
--
-- `shelves` has two child tables (`shelf_rules`, `shelf_books`) with
-- `ON DELETE CASCADE` FKs. The migrator runs with `foreign_keys = ON`, so a
-- bare `DROP TABLE shelves` would implicit-DELETE every shelf row and cascade
-- that delete into both children — wiping all rules and hand-picked membership.
-- To stay FK-safe we rebuild all three tables, preserving ids: the children are
-- recreated pointing at the new parent *before* the old tables are dropped, so
-- no drop ever cascades through a live FK.

-- --- new parent (widened kind CHECK) ----------------------------------------
CREATE TABLE shelves_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('smart','manual','wishlist')),
    name          TEXT    NOT NULL,
    description   TEXT,
    visibility    TEXT    NOT NULL DEFAULT 'private'
                          CHECK (visibility IN ('private','public')),
    match_mode    TEXT    CHECK (match_mode IN ('any','all')),
    accent        TEXT,
    icon          TEXT,
    position      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at    INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
INSERT INTO shelves_new SELECT * FROM shelves;

-- --- new children, pointing at shelves_new ----------------------------------
CREATE TABLE shelf_rules_new (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    shelf_id  INTEGER NOT NULL REFERENCES shelves_new(id) ON DELETE CASCADE,
    field     TEXT    NOT NULL,
    op        TEXT    NOT NULL,
    value     TEXT    NOT NULL,
    position  INTEGER NOT NULL DEFAULT 0
);
INSERT INTO shelf_rules_new SELECT * FROM shelf_rules;

CREATE TABLE shelf_books_new (
    shelf_id         INTEGER NOT NULL REFERENCES shelves_new(id) ON DELETE CASCADE,
    book_uuid        TEXT    NOT NULL,
    position         INTEGER NOT NULL DEFAULT 0,
    added_by_user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    added_at         INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (shelf_id, book_uuid)
);
INSERT INTO shelf_books_new SELECT * FROM shelf_books;

-- --- drop old (children first, so dropping the parent has no live child) -----
DROP TABLE shelf_rules;
DROP TABLE shelf_books;
DROP TABLE shelves;

-- --- rename into place (FK refs auto-repoint shelves_new -> shelves) ---------
ALTER TABLE shelves_new     RENAME TO shelves;
ALTER TABLE shelf_rules_new RENAME TO shelf_rules;
ALTER TABLE shelf_books_new RENAME TO shelf_books;

-- --- indexes ----------------------------------------------------------------
CREATE INDEX idx_shelves_owner_vis ON shelves(owner_user_id, visibility);
CREATE INDEX idx_shelf_rules_shelf ON shelf_rules(shelf_id);

-- The per-owner name uniqueness is a *user-shelf* rule; the system Wishlist
-- shelf is exempt so a user who already owns a manual shelf named "Wishlist"
-- can still be provisioned. Partial index excludes the wishlist kind.
CREATE UNIQUE INDEX idx_shelves_owner_name
    ON shelves(owner_user_id, name COLLATE NOCASE)
    WHERE kind <> 'wishlist';

-- Exactly one wishlist shelf per owner (AC1 idempotency, enforced in schema so
-- a double-provision is a no-op conflict rather than a duplicate row).
CREATE UNIQUE INDEX idx_shelves_owner_wishlist
    ON shelves(owner_user_id)
    WHERE kind = 'wishlist';

-- Provision the shelf for every existing user. The boot backfill
-- (`provision_wishlist_shelves`) re-runs this idempotently on every start, and
-- `create_user` provisions new registrations inline; this seeds current users
-- at migrate time so they have it immediately.
INSERT INTO shelves (owner_user_id, kind, name, visibility)
SELECT id, 'wishlist', 'Wishlist', 'public' FROM users
WHERE NOT EXISTS (
    SELECT 1 FROM shelves s WHERE s.owner_user_id = users.id AND s.kind = 'wishlist'
);
