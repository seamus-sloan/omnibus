//! Progress tracking and eviction: `Worker::progress_snapshot` and its
//! owner-scoped sibling, the test-only retention seam, the internal
//! `write_terminal_progress` invoked from both [`super::exec`] and the panic
//! guard in [`super::queue`], and the mid-task report seams —
//! `report_progress_update` (counted state + detail) and `report_detail`.

use std::time::{Duration, Instant};

use omnibus_shared::{ProgressState, TaskDetail, WorkerStatus};

use super::types::{lock_unpoison, wall_clock_ms, TaskId, Worker, TERMINAL_RETENTION};

impl Worker {
    /// Snapshot of every live worker task — non-terminal entries go in
    /// `active`, terminal entries (`Done` / `Failed`) less than
    /// [`TERMINAL_RETENTION`] old go in `recent_complete`. Older terminal
    /// entries are evicted under the same lock. Both vecs are sorted by
    /// `task_id` so a polling client renders a stable list across ticks.
    ///
    /// Unfiltered — includes every user's owned tasks. Only safe to call
    /// from a context that doesn't forward the result to an untrusted
    /// caller (tests, the periodic-scan seam); an RPC handler serving a
    /// polling client should use [`Worker::owner_scoped_snapshot`] instead.
    pub fn progress_snapshot(&self) -> WorkerStatus {
        self.progress_snapshot_with_retention(TERMINAL_RETENTION)
    }

    /// Owner-scoped variant of [`Worker::progress_snapshot`] for a
    /// polling RPC handler: keeps every task with no recorded owner
    /// (shared library-wide work — a scan, a thumbnail regen, an
    /// author-photo refetch) plus entries owned by `user_id`, and drops
    /// every other user's owned task. Closes the leak where any
    /// authenticated user could read another user's Send-to-Kindle
    /// failure text off the general worker-status poll, bypassing the
    /// owner check [`Worker::owned_task_state`] already enforces on the
    /// dedicated Kindle-status poll.
    pub fn owner_scoped_snapshot(&self, user_id: i64) -> WorkerStatus {
        self.snapshot(TERMINAL_RETENTION, Some(user_id))
    }

    /// Test-friendly variant of [`Worker::progress_snapshot`] that lets
    /// callers supply a custom retention window. Production code always
    /// uses [`TERMINAL_RETENTION`]; the unit-test suite passes a much
    /// shorter window so the eviction assertion doesn't have to sleep
    /// for the full 10 s of wall-clock time.
    pub(super) fn progress_snapshot_with_retention(&self, retention: Duration) -> WorkerStatus {
        self.snapshot(retention, None)
    }

    /// Shared snapshot body for [`Worker::progress_snapshot`],
    /// [`Worker::owner_scoped_snapshot`], and
    /// [`Worker::progress_snapshot_with_retention`]. `viewer` narrows the
    /// result to unowned entries plus that user's own when set; `None`
    /// returns everything.
    fn snapshot(&self, retention: Duration, viewer: Option<i64>) -> WorkerStatus {
        let now = Instant::now();
        let mut map = lock_unpoison(&self.progress);

        // First pass: identify expired terminals so we don't hold the
        // iterator while mutating the map.
        let expired: Vec<TaskId> = map
            .iter()
            .filter_map(|(id, entry)| match entry.terminal_at {
                Some(at) if now.saturating_duration_since(at) >= retention => Some(*id),
                _ => None,
            })
            .collect();
        for id in expired {
            map.remove(&id);
        }

        let mut active = Vec::new();
        let mut recent_complete = Vec::new();
        for entry in map.values() {
            if let Some(user_id) = viewer {
                if entry.owner.is_some_and(|owner| owner != user_id) {
                    continue;
                }
            }
            if entry.terminal_at.is_some() {
                recent_complete.push(entry.progress.clone());
            } else {
                active.push(entry.progress.clone());
            }
        }
        // BTreeMap iteration is already key-ordered, so both vecs come out
        // sorted by `task_id` without an explicit sort.
        WorkerStatus {
            active,
            recent_complete,
        }
    }

    /// Internal terminal write. Called from `run` (Ok/Err) and from the
    /// `ProgressTerminalGuard` (panic). The `terminal_at` Instant is
    /// monotonic so eviction is robust to wall-clock drift. Also records
    /// this task's wall-clock run duration into
    /// [`Worker::record_completion`]'s per-kind window, backing
    /// [`Worker::metrics`] — the two mutexes involved (`progress` and
    /// `completion_timings`) are never held at once, so this can't
    /// introduce a lock-order deadlock with `metrics()`.
    pub(super) fn write_terminal_progress(&self, id: TaskId, state: ProgressState) {
        let recorded = {
            let mut map = lock_unpoison(&self.progress);
            map.get_mut(&id).map(|entry| {
                let now_ms = wall_clock_ms();
                entry.progress.last_update_ms = now_ms;
                entry.progress.state = state;
                // Phase and current-item are live-progress facts that would
                // read as stale on a terminal row; the tallies are the scan's
                // final summary, so they ride onto the `Done` entry for the
                // completion banner.
                if let Some(detail) = entry.progress.detail.as_mut() {
                    detail.phase = None;
                    detail.current_item = None;
                    if detail.is_empty() {
                        entry.progress.detail = None;
                    }
                }
                entry.terminal_at = Some(Instant::now());
                let elapsed_ms = now_ms.saturating_sub(entry.progress.started_at_ms).max(0);
                (
                    entry.progress.kind,
                    Duration::from_millis(elapsed_ms as u64),
                )
            })
        };
        if let Some((kind, duration)) = recorded {
            self.record_completion(kind, duration);
        }
    }

    /// Write the in-flight progress count for `id`, plus a full
    /// [`TaskDetail`] replacement, in one lock acquisition. The terminal
    /// state is written separately at the end of [`Worker::run`] so a
    /// mid-task report can't accidentally flip a task to `Done`. An empty
    /// detail clears the field rather than storing `Some(empty)`.
    pub(crate) fn report_progress_update(
        &self,
        id: TaskId,
        processed: u32,
        total: Option<u32>,
        detail: TaskDetail,
    ) {
        let mut map = lock_unpoison(&self.progress);
        if let Some(entry) = map.get_mut(&id) {
            if entry.terminal_at.is_some() {
                return; // race: terminal already recorded
            }
            entry.progress.state = ProgressState::Running { processed, total };
            entry.progress.detail = (!detail.is_empty()).then_some(detail);
            entry.progress.last_update_ms = wall_clock_ms();
        }
    }

    /// Replace only the [`TaskDetail`] for `id`, leaving the counted state
    /// untouched. For task kinds that name their current item once at start
    /// (a single book's thumbnail, one author's photo) without a
    /// processed/total surface.
    pub(crate) fn report_detail(&self, id: TaskId, detail: TaskDetail) {
        let mut map = lock_unpoison(&self.progress);
        if let Some(entry) = map.get_mut(&id) {
            if entry.terminal_at.is_some() {
                return; // race: terminal already recorded
            }
            entry.progress.detail = (!detail.is_empty()).then_some(detail);
            entry.progress.last_update_ms = wall_clock_ms();
        }
    }
}
