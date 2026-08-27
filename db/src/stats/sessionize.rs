//! Stitching of session checkpoint rows back into sittings. Every client
//! flushes a running segment on a timer, so one sitting lands as many rows;
//! [`stitched`] regroups them at query time so callers count sittings rather
//! than flush cadence. Read-side only — the writers stay as they are.

/// Idle seconds that end a sitting. Comfortably above every client's flush
/// interval — the longest is the iOS reader's 300s checkpoint — so no
/// cadence can manufacture a split and only real idleness does.
pub(super) const IDLE_GAP_SECS: i64 = 900;

/// Shortest sitting that counts as a session. Opening a book and closing it
/// again is a glance, not a sitting, and the writers' own 5s floor is far too
/// low to filter one.
///
/// This bounds the **count** only: every second stays in the seconds totals,
/// which are what "Time read" reports. Filtering the totals too would mean
/// deleting time the user genuinely spent, and an inflated mean is a far
/// smaller lie than an under-reported total. The floor lives here rather than
/// at the writers so it applies to already-recorded rows, keeps a glance's
/// seconds instead of discarding them, and stays one number rather than four
/// (web, two iOS readers, the iOS player) that can drift apart.
pub(super) const MIN_SITTING_SECS: i64 = 60;

/// Wrap a session-row union in the gap-and-islands grouping, yielding one
/// `(book_uuid, started_at, secs)` row per stitched sitting: the book, the
/// first row's start, and the total seconds recorded across the sitting.
///
/// `inner` must select `book_uuid, started_at, ended_at, secs` and carries
/// its own binds — this adds none, since the threshold is a const literal.
///
/// Sittings are scoped per book, so the user-wide count stays the sum of
/// every book's and the two surfaces can't disagree. Within a book the union
/// spans both session tables, so a dual-format book read and listened to in
/// one stretch is one sitting rather than one per table.
pub(super) fn stitched(inner: &str) -> String {
    // The break test compares against a running MAX of every earlier row's
    // `ended_at`, not LAG's immediately-preceding one: rows overlap when a
    // book is read and listened to at once, and the row before by
    // `started_at` can then end earlier than one before it, which LAG would
    // read as an idle gap and split a single sitting on.
    format!(
        "SELECT book_uuid, MIN(started_at) AS started_at, SUM(secs) AS secs FROM (
             SELECT book_uuid, started_at, secs,
                    SUM(brk) OVER (
                        PARTITION BY book_uuid ORDER BY started_at, ended_at
                        ROWS UNBOUNDED PRECEDING
                    ) AS sitting
             FROM (
                 SELECT book_uuid, started_at, ended_at, secs,
                        CASE WHEN started_at - MAX(ended_at) OVER (
                                 PARTITION BY book_uuid ORDER BY started_at, ended_at
                                 ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                             ) > {IDLE_GAP_SECS} THEN 1 ELSE 0 END AS brk
                 FROM ({inner})
             )
         ) GROUP BY book_uuid, sitting"
    )
}
