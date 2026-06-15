-- F2 — Durable book identity: stored UUID + relative-path scan key.
--
-- `books.uuid` stops being a path-derived value and becomes a durable,
-- minted-once identity (a fresh UUIDv4 at insert, never recomputed). Existing
-- `uuid` values are already unique and are kept verbatim, so nothing re-keys:
-- every cover file, `merged_uuids` row, `metadata_overrides`, and
-- `/api/covers/{uuid}` URL stays valid.
--
-- A new `scan_key` takes over the Phase-A filesystem diff. It stores the
-- book's path *relative to the scan root* (e.g. `Author/Title.epub`), so a
-- library-root repoint — which leaves every relative path unchanged — matches
-- every row and preserves every uuid instead of re-minting them. `library_id`
-- is the existing column; the diff is scoped per scan root, so the relative
-- path alone is the match key and `(library_id, scan_key)` is unique.
--
-- Additive and non-destructive: nullable columns + a partial unique index.
-- The boot backfill (`identity::backfill_scan_keys`) fills `scan_key` for
-- pre-migration rows purely from the stored `path`/`book_files` columns — no
-- filesystem reads — mirroring `normalize::backfill_norm_columns`.

ALTER TABLE books ADD COLUMN scan_key TEXT;

-- Durable identity (`books.uuid`) is already UNIQUE; this enforces that the
-- relative-path diff key is unique per scan root. NULLs (pre-backfill, or a
-- fileless ghost that never had a path) are exempt via the partial predicate.
CREATE UNIQUE INDEX idx_books_scan_key
    ON books(library_id, scan_key) WHERE scan_key IS NOT NULL;

-- The cross-format attach ledger keys attached secondary files by uuid today;
-- that uuid is path-derived and so also moves on a repoint. Give it the same
-- relative-path key so attached files survive a repoint alongside their book.
ALTER TABLE merged_uuids ADD COLUMN scan_key TEXT;
