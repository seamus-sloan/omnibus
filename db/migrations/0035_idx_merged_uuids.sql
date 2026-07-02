-- Secondary indexes for the `merged_uuids` hot lookups.
--
-- Migration 0026 added `scan_key` to `merged_uuids` and made the
-- `(library_path, scan_key)` pair the reindex diff key for cross-format
-- attachments, but shipped no accompanying index. The only pre-existing
-- indexes are the `uuid` PK and `idx_merged_uuids_book (book_id)`, so every
-- `find_attachment_by_scan_key` / `record_attachment` call in
-- `db/src/sync/attach/mod.rs` scans the whole ledger, and
-- `list_merged_rows_for_formats` in `db/src/books/list.rs` scans it once per
-- library-scoped landing/browse query.
--
-- These indexes let the planner seek both patterns directly:
--   * `(library_path, scan_key)` — index-time attach lookup, keyed once per
--     scanned file on every reindex cycle
--   * `(library_path, format)` — landing/browse projection joining
--     `merged_uuids -> book_files` for the format-filtered rail
--
-- Forward-only, pure secondary indexes — no backfill, no table recreate,
-- so a fresh DB and an upgraded DB converge on the same schema.
-- `IF NOT EXISTS` keeps the migration idempotent against any hand-created
-- index of the same name.
CREATE INDEX IF NOT EXISTS idx_merged_uuids_library_scan
    ON merged_uuids(library_path, scan_key);

CREATE INDEX IF NOT EXISTS idx_merged_uuids_library_format
    ON merged_uuids(library_path, format);
