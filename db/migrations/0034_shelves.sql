-- Shelves (F3.1): named subsets of the library shown in the Library rail.
-- Two kinds — 'smart' (membership computed from a field/op/value rule over
-- normalized metadata, auto-updating) and 'manual' (an explicit, ordered list).
-- Owned by a user; visible to owner + admins, or everyone when 'public'.
CREATE TABLE shelves (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('smart','manual')),
    name          TEXT    NOT NULL,
    description   TEXT,
    visibility    TEXT    NOT NULL DEFAULT 'private'
                          CHECK (visibility IN ('private','public')),
    -- 'any' (OR) or 'all' (AND) across smart conditions; NULL for manual shelves.
    match_mode    TEXT    CHECK (match_mode IN ('any','all')),
    accent        TEXT,                                  -- CSS accent, auto-assigned
    icon          TEXT,                                  -- reserved; kind implies marker
    position      INTEGER NOT NULL DEFAULT 0,            -- reserved for rail reorder
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at    INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

-- The rail lists every visible shelf on each Library load — index the filter.
CREATE INDEX idx_shelves_owner_vis ON shelves(owner_user_id, visibility);
-- A shelf name is unique per owner (case-insensitive); two different users can
-- each have a "Favorites".
CREATE UNIQUE INDEX idx_shelves_owner_name
    ON shelves(owner_user_id, name COLLATE NOCASE);

-- Smart-shelf conditions. `field`/`op`/`value` mirror the F1.5 advanced-search
-- shape; `value` interpretation is per-field:
--   author/series → id; tag/status/format → string; year/rating → int;
--   date_added/date_updated → ISO 'YYYY-MM-DD', range 'START..END',
--                             or relative '30d' | '3m' | '1y'.
CREATE TABLE shelf_rules (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    shelf_id  INTEGER NOT NULL REFERENCES shelves(id) ON DELETE CASCADE,
    field     TEXT    NOT NULL,   -- tag|author|series|rating|status|format|year|date_added|date_updated
    op        TEXT    NOT NULL,   -- is|is_not|gte|includes|in_last|between|before|after
    value     TEXT    NOT NULL,
    position  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_shelf_rules_shelf ON shelf_rules(shelf_id);

-- Hand-picked membership. `book_uuid` is a SOFT reference to the durable
-- `books.uuid` (no FK, no cascade) so a reindex / scan-root repoint / format
-- merge keeps the shelf intact; a ghosted book stays until library cleanup.
-- A book on N shelves is N rows. `position` backs drag-to-reorder.
CREATE TABLE shelf_books (
    shelf_id         INTEGER NOT NULL REFERENCES shelves(id) ON DELETE CASCADE,
    book_uuid        TEXT    NOT NULL,
    position         INTEGER NOT NULL DEFAULT 0,
    added_by_user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    added_at         INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (shelf_id, book_uuid)
);
