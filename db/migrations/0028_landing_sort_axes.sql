-- F5b — denormalized series sort key + per-axis composite indexes for the
-- keyset-paginated, server-side-sorted landing/browse read path.
--
-- F5b moves the landing list off "hydrate the whole library, sort in Rust"
-- onto a keyset cursor whose ORDER BY the server owns. Four of the five sort
-- axes already map to real `books` columns (title `sort`, `author_sort`,
-- `last_modified`, `timestamp`); each gets a `(library_id, <col>, id)`
-- composite so the planner can seek the library filter and walk the axis in
-- order — the same shape as 0025's `(library_id, sort, id)` for the Title
-- axis, which this migration deliberately does NOT duplicate.
--
-- The Series axis is the exception: a book's series is a m2m relation
-- (`books_series_link` -> `series.name`) with no column on `books` to seek.
-- Following the existing `author_sort` precedent (a denormalized sort key
-- carried on the row), add `series_sort` — the primary series name, copied
-- from scanned metadata at index time by the sync writers and backfilled once
-- at boot for pre-existing rows (`sort_keys::backfill_series_sort`). The
-- `(library_id, series_sort, series_index, id)` composite then serves a series
-- sort (by name, then by in-series index) as an index range scan.
--
-- COLLATE NOCASE on `series_sort` matches the column collation 0002 declares
-- on `sort` / `author_sort`, so the keyset comparison and the index agree
-- (SQLite uses the column's declared collation for the index automatically).
--
-- Forward-only, append-only. `series_sort` is nullable with no default — a
-- seriesless book sorts under SQLite's natural NULL ordering, and the boot
-- backfill only touches rows that actually have a series link, so it is a
-- true no-op once caught up. The indexes are pure secondary indexes
-- (populated on CREATE; no table recreate, no column types/PKs/constraints
-- change), so a fresh DB built from 0001..0028 and an upgraded DB converge.
-- Like 0025's index, these depend on the cursor comparison running under the
-- columns' NOCASE collation.

ALTER TABLE books ADD COLUMN series_sort TEXT COLLATE NOCASE;

CREATE INDEX IF NOT EXISTS idx_books_library_author_sort
    ON books (library_id, author_sort, id);

CREATE INDEX IF NOT EXISTS idx_books_library_last_modified
    ON books (library_id, last_modified, id);

CREATE INDEX IF NOT EXISTS idx_books_library_timestamp
    ON books (library_id, timestamp, id);

CREATE INDEX IF NOT EXISTS idx_books_library_series_sort
    ON books (library_id, series_sort, series_index, id);
