-- Scope the forward-progress ledger's accrual to a reading *sitting*.
--
-- Migration `0083` differenced every position write against the last percent
-- observed, clamping a backward move to zero but still letting the mark follow
-- the reader back. Re-covering that ground then accrued a second time: at 40%,
-- flipping back to 30% to find a quote and returning to 45% charged 15 where
-- only 5 was new. It measured the *path* the reader walked, not the ground they
-- covered, so any backtracking inflated the Pages read tile and everything
-- derived from it.
--
-- The mark now holds the furthest point reached in the current sitting and
-- accrues only above it, so a there-and-back move inside one sitting is free.
-- A sitting ends after `stats::sessionize::IDLE_GAP_SECS` of no observation,
-- and the next one baselines wherever the reader then is — which is what keeps
-- a deliberate re-read counting in full: restarting a finished book opens a new
-- sitting at 5%, not a doomed climb back to a lifetime high-water of 100.
--
-- This is BookOrbit's session-delta semantics reached from the other side. They
-- bracket a session on the client and post one `end - start` delta; that shape
-- was rejected for omnibus in #2139 because every reading surface would have to
-- learn to carry a percent first (the iOS reader posts a CFI and no percent at
-- all), and a session lost on a crashed tab takes its whole delta with it.
-- Deriving the same bracket from the position writes the clients already make
-- needs no client change and inherits the offline outboxes' durability.
--
-- Known residual, deliberate: a backtrack spanning a sitting boundary — go
-- back, put the book down, read forward tomorrow — still over-credits, because
-- a per-sitting clamp cannot net across sittings. Closing that needs signed
-- deltas or an explicit re-read boundary; see the discussion on #2394.

-- Renamed rather than added: it is the same slot, holding the same kind of
-- value, under one new rule — it no longer follows the reader backward mid
-- sitting. A second percent column would just be two answers to one question.
-- SQLite rewrites the column's CHECK constraint along with the rename.
ALTER TABLE reading_progress_marks RENAME COLUMN percent TO sitting_max_percent;

-- The sitting clock, deliberately *not* `updated_at`. Two differences, and both
-- matter: this one advances only on a real observation, where `updated_at`
-- carries `DEFAULT (strftime('%s','now'))` and is what `merge::transaction`'s
-- `dedupe_latest_wins` arbitrates on — so a merge or a maintenance write that
-- touched the row would silently end or extend a sitting if the two were one
-- column. And this one is clamped forward monotonically, so an observation that
-- arrives out of order (the off-request-path percent derivation landing late
-- with the position's older event time) cannot drag the boundary backward and
-- manufacture a gap.
--
-- Nullable with a backfill rather than `NOT NULL DEFAULT`: SQLite cannot add a
-- NOT NULL column without a constant default, and there is no honest constant
-- here. NULL reads as "no sitting in progress", which re-baselines on the next
-- observation — the right answer for every existing row, since none of them is
-- mid-sitting across a deploy anyway.
ALTER TABLE reading_progress_marks ADD COLUMN sitting_observed_at INTEGER;

UPDATE reading_progress_marks SET sitting_observed_at = updated_at;
