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

-- `book_read_status` and `user_ratings` are UNIQUE (user_id, book_uuid), so
-- every row that will land on the same surviving book for the same reader has
-- to be resolved down to one before the retarget runs.
--
-- Note the plural: a chain of merges (A into B, then B into C) leaves *several*
-- orphan uuids pointing at one live book, so this is not merely orphan-vs-live.
-- A reader who rated two books that were later merged into the same third one
-- has two orphan rows plus possibly a live one, and repointing them all would
-- violate the UNIQUE constraint and abort the upgrade. Resolving pairwise is
-- what the runtime `dedupe_latest_wins` can afford to do — it only ever moves
-- one source onto one target — and is not enough here.
--
-- So: resolve each candidate row to the uuid it will end up on, then delete
-- every row that has a strictly better sibling in that final group. Latest
-- `updated_at` wins; ties go to the row already on the surviving book, then to
-- the lowest id, so the outcome is deterministic and exactly one row survives
-- per (user, surviving book).
CREATE TEMP TABLE merge_orphan_resolved AS
SELECT 'book_read_status' AS src, r.id AS id, r.user_id AS user_id,
       r.updated_at AS updated_at,
       COALESCE(m.live_uuid, r.book_uuid) AS target_uuid,
       (m.live_uuid IS NULL) AS on_live
  FROM book_read_status r
  LEFT JOIN merge_orphan_map m ON m.orphan_uuid = r.book_uuid
 WHERE r.book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map)
    OR r.book_uuid IN (SELECT live_uuid FROM merge_orphan_map)
UNION ALL
SELECT 'user_ratings', r.id, r.user_id, r.updated_at,
       COALESCE(m.live_uuid, r.book_uuid), (m.live_uuid IS NULL)
  FROM user_ratings r
  LEFT JOIN merge_orphan_map m ON m.orphan_uuid = r.book_uuid
 WHERE r.book_uuid IN (SELECT orphan_uuid FROM merge_orphan_map)
    OR r.book_uuid IN (SELECT live_uuid FROM merge_orphan_map);

DELETE FROM book_read_status
 WHERE id IN (
   SELECT x.id FROM merge_orphan_resolved x
    WHERE x.src = 'book_read_status'
      AND EXISTS (SELECT 1 FROM merge_orphan_resolved o
                   WHERE o.src = x.src AND o.user_id = x.user_id
                     AND o.target_uuid = x.target_uuid AND o.id <> x.id
                     AND (o.updated_at > x.updated_at
                       OR (o.updated_at = x.updated_at AND o.on_live > x.on_live)
                       OR (o.updated_at = x.updated_at AND o.on_live = x.on_live
                           AND o.id < x.id))));

DELETE FROM user_ratings
 WHERE id IN (
   SELECT x.id FROM merge_orphan_resolved x
    WHERE x.src = 'user_ratings'
      AND EXISTS (SELECT 1 FROM merge_orphan_resolved o
                   WHERE o.src = x.src AND o.user_id = x.user_id
                     AND o.target_uuid = x.target_uuid AND o.id <> x.id
                     AND (o.updated_at > x.updated_at
                       OR (o.updated_at = x.updated_at AND o.on_live > x.on_live)
                       OR (o.updated_at = x.updated_at AND o.on_live = x.on_live
                           AND o.id < x.id))));

DROP TABLE merge_orphan_resolved;

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
