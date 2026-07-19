-- F5.1: per-library metadata source precedence (#972). An ordered JSON
-- array of source names on each scan_roots row, walked by the merge path
-- to resolve which source wins per field instead of the old hardcoded
-- override-always-wins order. `NOT NULL DEFAULT` backfills every existing
-- row automatically (no boot-time backfill needed, unlike the nullable
-- `_norm`/`scan_key` columns that require re-deriving from other data).
--
-- List order is lowest-to-highest priority (last entry wins a field, CSS-
-- cascade style). The default reproduces today's behavior: embedded_tags
-- (the scanned OPF/tag metadata) loses to omnibus_overrides (the F5.1
-- user-edit layer), same as the current hardcoded `apply_overrides`.
ALTER TABLE scan_roots ADD COLUMN metadata_precedence TEXT NOT NULL
    DEFAULT '["folder_structure","embedded_tags","opf_sidecar","omnibus_overrides","provider_match"]';
