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
-- Nulling a key is how a row is handed back to `normalize::backfill_norm_columns`
-- (it only touches rows where `title_norm IS NULL`), which recomputes from the
-- same `books.title` and position-0 author link the sync writers derive from.
-- Only ampersand-bearing rows change key, so only they are reset — a blanket
-- reset would re-derive rows the change never touched.
--
-- Two arms, because the two keys stale independently and one of them is not
-- always re-derivable:
--
--   1. A `&` in the **title** stales the title key only. It must NOT null
--      `author_norm`: the backfill derives that from the position-0 author
--      link, and an `ignored_authors` blocklist entry on the first creator
--      leaves that link absent (the sync writer keys off `creators.first()`
--      blocklisted or not), so the value would be unrecoverable. The backfill
--      COALESCEs a non-derivable recompute over the stored value, so leaving
--      it alone here is what preserves it.
--   2. A `&` in the **position-0 author's name** stales the author key. Only
--      position 0 feeds `author_norm`, so the EXISTS is pinned to it rather
--      than scanning every link. `title_norm` is nulled too — not because the
--      title staled, but because it is the backfill's trigger; the recomputed
--      title value is identical. The link exists by construction here, so the
--      author recompute cannot come back non-derivable.
--
-- Residuals, both healing on the book's next reindex and both undetectable
-- from SQL:
--   * A blocklisted first creator whose *name* carries the `&` has no `authors`
--     row to match on (blocklisted names are never upserted), so arm 2 cannot
--     see it.
--   * A codepoint that only becomes `&` after `deunicode` folding is invisible
--     to a `LIKE '%&%'`.
--
-- `metadata_overrides.(title_norm, author_norm)` need no reset:
-- `backfill_override_norm_columns` already recomputes every row from its
-- `overrides` JSON on each boot and rewrites the ones that disagree.

-- Arm 1: ampersand in the title.
UPDATE books
   SET title_norm = NULL
 WHERE title LIKE '%&%';

-- Arm 2: ampersand in the position-0 author's name.
UPDATE books
   SET title_norm  = NULL,
       author_norm = NULL
 WHERE EXISTS (
        SELECT 1
          FROM books_authors_link l
          JOIN authors a ON a.id = l.author
         WHERE l.book = books.id
           AND l.position = 0
           AND a.name LIKE '%&%'
       );
