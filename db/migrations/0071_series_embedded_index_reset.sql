-- #1912: series metadata that carries its index inside the name ("Mistborn
-- #2", "Mistborn, Book 2") used to fragment into one one-book series per
-- index instead of a single series with per-book indexes. The sync writers
-- now strip that suffix into `books.series_index` and store the cleaned
-- name (`helpers::split_embedded_series_index`), and
-- `series_normalize::backfill_embedded_series_index` merges any existing
-- fragmented `series` rows into the cleaned canonical one and repoints
-- `books_series_link` accordingly.
--
-- That merge alone leaves `books.series_sort` stale for a book indexed
-- before this fix: `series_sort_value` denormalized the *raw*,
-- unmerged name onto the row at scan time, and it is not NULL, so the
-- existing `sort_keys::backfill_series_sort` (guarded on `series_sort IS
-- NULL`) would never revisit it. Nulling it here hands the row back to that
-- pass, which reads the merged link the Rust backfill leaves behind and
-- recomputes `series_sort` from the cleaned name.
--
-- `books.series_index` needs no reset: a book relying on an embedded index
-- never had an explicit one, so it is already NULL — exactly the state
-- `backfill_embedded_series_index`'s own `WHERE series_index IS NULL` guard
-- looks for.
--
-- The match below is a deliberate superset, not the precise regex the Rust
-- side uses (SQLite has no native regex support): any linked series name
-- containing '#' or the word "book" is reset. A false positive (a series
-- genuinely named e.g. "Book of the New Sun") costs one harmless no-op
-- recompute — `backfill_series_sort` just restores the same value. `series.name`
-- is `COLLATE NOCASE` (migration 0002), so this LIKE is already
-- case-insensitive.
UPDATE books
   SET series_sort = NULL
 WHERE EXISTS (
        SELECT 1
          FROM books_series_link bsl
          JOIN series s ON s.id = bsl.series
         WHERE bsl.book = books.id
           AND (s.name LIKE '%#%' OR s.name LIKE '%book%')
       );
