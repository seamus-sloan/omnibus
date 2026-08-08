-- Library Cleanup persistence foundation (#961): dedup-suggestion detection
-- results, an undo log for applied cleanups, and the merged-away name ledger
-- that keeps a dedup decision from being suggested again. This migration is
-- schema-only — the detectors, apply/undo, and review UI are separate
-- follow-up issues (#962-#966) that write and read these tables.

-- One row per detected dedup opportunity. `kind`/`action`/`payload_json` are
-- opaque TEXT on the DB side (typed as `CleanupKind`/`CleanupAction` in
-- `shared::cleanup` on the wire) — the detectors own the payload shape, this
-- table just persists and de-duplicates it. The UNIQUE constraint is the
-- point: re-running detection must never insert a suggestion that's already
-- pending (or already decided), so a periodic detector job can be a plain
-- `INSERT OR IGNORE` with no separate existence check.
CREATE TABLE dedup_suggestions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    kind         TEXT    NOT NULL,
    action       TEXT    NOT NULL,
    payload_json TEXT    NOT NULL,
    decision     TEXT    NOT NULL DEFAULT 'pending' CHECK (decision IN ('pending', 'accepted', 'rejected')),
    created_at   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    decided_at   INTEGER,
    decided_by   INTEGER REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE (kind, action, payload_json)
);
CREATE INDEX idx_dedup_suggestions_decision ON dedup_suggestions(decision);

-- Audit log for *applied* cleanups, mirroring `merge_log` (0016): a
-- self-contained `snapshot_json` blob is what a future undo replays from, so
-- undo never has to reconstruct pre-cleanup state from the current schema.
-- Soft reference to the suggestion it came from (SET NULL, not CASCADE) —
-- the log entry must survive a suggestion row being pruned.
CREATE TABLE cleanup_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    suggestion_id INTEGER REFERENCES dedup_suggestions(id) ON DELETE SET NULL,
    kind          TEXT    NOT NULL,
    action        TEXT    NOT NULL,
    snapshot_json TEXT    NOT NULL,
    applied_by    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    applied_at    INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    undone_at     INTEGER
);
CREATE INDEX idx_cleanup_log_suggestion ON cleanup_log(suggestion_id);

-- Merged-away name -> surviving entity ledger, keyed on the name that no
-- longer resolves directly. `kind` scopes `alias_name` to its own namespace
-- (author/series/tag names aren't unique across kinds); `canonical_id` is
-- the surviving row's id in that kind's own table (e.g. `authors.id`) — no
-- FK, since the target table varies by `kind` and SQLite has no polymorphic
-- foreign key.
CREATE TABLE entity_aliases (
    kind         TEXT    NOT NULL,
    alias_name   TEXT    NOT NULL,
    canonical_id INTEGER NOT NULL,
    created_at   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    PRIMARY KEY (kind, alias_name)
);
