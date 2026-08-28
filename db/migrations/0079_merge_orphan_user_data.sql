-- Repoint per-user rows that a past merge stranded on a deleted book.
--
-- `merge::transaction::move_progress_and_history` retargeted seven soft-ref
-- tables onto the surviving book but not `journal_entries`, `book_read_status`
-- or `user_ratings`, and `finalize_merge` then deleted the source `books` row.
-- Those rows are still on disk under a uuid no book carries, which makes them
-- invisible wherever a query joins `books` (the book page, the Finished tile,
-- the finished rail) and still countable wherever one doesn't (the trailing-12
-- chart, the vs-previous delta, the average-rating mean). The merge path is
-- fixed alongside this file; this heals the rows it already stranded.
--
-- Forward-only and self-limiting: `merged_uuids.uuid` is the primary key, so
-- each orphan resolves to exactly one survivor, and a database that has never
-- merged builds an empty map and no-ops through every statement below.

-- Orphans only: a uuid the attach ledger maps to a live book, that no `books`
-- row carries itself. The `<>` guard keeps a book that merely records its own
-- uuid in the ledger out of the map.
CREATE TEMP TABLE merge_orphan_map AS
SELECT mu.uuid AS orphan_uuid, b.uuid AS live_uuid
  FROM merged_uuids mu
  JOIN books b ON b.id = mu.book_id
 WHERE mu.uuid <> b.uuid
   AND NOT EXISTS (SELECT 1 FROM books o WHERE o.uuid = mu.uuid);

-- `book_read_status` and `user_ratings` are UNIQUE (user_id, book_uuid), so a
-- reader who rated (or finished) both sides of a merge has two rows that the
-- retarget would collide on. Resolve latest-wins, ties to the surviving book —
-- the same rule `dedupe_latest_wins` applies at merge time.
--
-- Two passes, in this order: drop the orphan rows the live row already beats,
-- then drop the live rows whose orphan counterpart outlived that first pass.
DELETE FROM book_read_status
 WHERE book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map)
   AND EXISTS (SELECT 1 FROM book_read_status live
                 JOIN merge_orphan_map m ON m.live_uuid = live.book_uuid
                WHERE m.orphan_uuid = book_read_status.book_uuid
                  AND live.user_id = book_read_status.user_id
                  AND live.updated_at >= book_read_status.updated_at);

DELETE FROM book_read_status
 WHERE book_uuid IN (SELECT live_uuid FROM merge_orphan_map)
   AND EXISTS (SELECT 1 FROM book_read_status orph
                 JOIN merge_orphan_map m ON m.orphan_uuid = orph.book_uuid
                WHERE m.live_uuid = book_read_status.book_uuid
                  AND orph.user_id = book_read_status.user_id);

DELETE FROM user_ratings
 WHERE book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map)
   AND EXISTS (SELECT 1 FROM user_ratings live
                 JOIN merge_orphan_map m ON m.live_uuid = live.book_uuid
                WHERE m.orphan_uuid = user_ratings.book_uuid
                  AND live.user_id = user_ratings.user_id
                  AND live.updated_at >= user_ratings.updated_at);

DELETE FROM user_ratings
 WHERE book_uuid IN (SELECT live_uuid FROM merge_orphan_map)
   AND EXISTS (SELECT 1 FROM user_ratings orph
                 JOIN merge_orphan_map m ON m.orphan_uuid = orph.book_uuid
                WHERE m.live_uuid = user_ratings.book_uuid
                  AND orph.user_id = user_ratings.user_id);

-- `journal_entries` has no per-book uniqueness (a reader keeps many entries on
-- one book), so every stranded row moves without a collision pass.
UPDATE journal_entries
   SET book_uuid = (SELECT m.live_uuid FROM merge_orphan_map m
                     WHERE m.orphan_uuid = journal_entries.book_uuid)
 WHERE book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map);

UPDATE book_read_status
   SET book_uuid = (SELECT m.live_uuid FROM merge_orphan_map m
                     WHERE m.orphan_uuid = book_read_status.book_uuid)
 WHERE book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map);

UPDATE user_ratings
   SET book_uuid = (SELECT m.live_uuid FROM merge_orphan_map m
                     WHERE m.orphan_uuid = user_ratings.book_uuid)
 WHERE book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map);

DROP TABLE merge_orphan_map;
