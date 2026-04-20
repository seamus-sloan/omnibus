-- Baseline schema for Omnibus. Verbatim lift from the former
-- `frontend::db::initialize_schema` so existing databases migrate in-place
-- (sqlx records this version as already-applied only when the checksum matches,
-- so for existing deployments this migration is effectively a no-op thanks to
-- the `IF NOT EXISTS` guards).

CREATE TABLE IF NOT EXISTS app_state (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    value INTEGER NOT NULL
);

INSERT INTO app_state (id, value)
SELECT 1, 0
WHERE NOT EXISTS (SELECT 1 FROM app_state WHERE id = 1);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS books (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    library_path      TEXT NOT NULL,
    filename          TEXT NOT NULL,
    title             TEXT,
    description       TEXT,
    publisher         TEXT,
    published         TEXT,
    modified          TEXT,
    language          TEXT,
    rights            TEXT,
    source            TEXT,
    coverage          TEXT,
    dc_type           TEXT,
    dc_format         TEXT,
    relation          TEXT,
    creators_json     TEXT NOT NULL DEFAULT '[]',
    contributors_json TEXT NOT NULL DEFAULT '[]',
    subjects_json     TEXT NOT NULL DEFAULT '[]',
    identifiers_json  TEXT NOT NULL DEFAULT '[]',
    series            TEXT,
    series_index      TEXT,
    epub_version      TEXT,
    unique_identifier TEXT,
    resource_count    INTEGER NOT NULL DEFAULT 0,
    spine_count       INTEGER NOT NULL DEFAULT 0,
    toc_count         INTEGER NOT NULL DEFAULT 0,
    has_cover         INTEGER NOT NULL DEFAULT 0,
    error             TEXT,
    UNIQUE(library_path, filename)
);

CREATE TABLE IF NOT EXISTS book_covers (
    book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    mime    TEXT NOT NULL,
    bytes   BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS library_index_state (
    library_path TEXT PRIMARY KEY,
    last_indexed INTEGER NOT NULL
);
