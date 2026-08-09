-- Re-derive the (title, author) match key now that `&` expands to `and`.
--
-- `db::normalize` used to drop `&` with the rest of the punctuation, so a
-- library book titled "Mirth & Magic" keyed as `mirth magic` while a
-- provider's "Mirth and Magic" keyed as `mirth and magic`. Check-In's norm
-- rung and the cross-format auto-attach both compare that key for equality
-- (or a word-boundary prefix), and the difference sits mid-string, so
-- neither could bridge it. `&` now expands, which leaves every stored key
-- derived from an ampersand-bearing title or author stale.
--
-- Null those keys so `normalize::backfill_norm_columns` — which only touches
-- rows where `title_norm IS NULL` and therefore would never heal them —
-- recomputes both columns on the next boot, from the same `books.title` and
-- position-0 author link the sync writers derive them from.
--
-- Only ampersand-bearing rows change key, so only they are reset. A blanket
-- reset would re-derive *every* book's author key from its position-0 author
-- link, which an `ignored_authors` blocklist entry can legitimately leave
-- absent (the sync writer keys off the first creator, blocklisted or not) —
-- turning an unrelated book un-attachable until its next reindex.
--
-- `metadata_overrides.(title_norm, author_norm)` need no reset:
-- `backfill_override_norm_columns` already recomputes every row from its
-- `overrides` JSON on each boot and rewrites the ones that disagree.
UPDATE books
   SET title_norm  = NULL,
       author_norm = NULL
 WHERE title LIKE '%&%'
    OR EXISTS (
        SELECT 1
          FROM books_authors_link l
          JOIN authors a ON a.id = l.author
         WHERE l.book = books.id
           AND a.name LIKE '%&%'
       );
