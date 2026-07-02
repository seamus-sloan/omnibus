-- Indexes for the two hot reindex/list queries against `merged_uuids`.
--
-- Every scanned file lands in `find_attachment_by_scan_key` /
-- `record_attachment` (both filter on `library_path = ? AND scan_key = ?`),
-- and multiformat listings hit `list_merged_rows_for_formats` (filter on
-- `library_path = ? AND format IN (...)`). Migration 0026 added
-- `scan_key` without an index and migration 0016 (the table itself) never
-- indexed `(library_path, format)` either, so both codepaths were doing
-- full table scans that grow with total attachment count.
CREATE INDEX IF NOT EXISTS idx_merged_uuids_library_scan
    ON merged_uuids(library_path, scan_key);

CREATE INDEX IF NOT EXISTS idx_merged_uuids_library_format
    ON merged_uuids(library_path, format);
