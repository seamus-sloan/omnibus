//! Aggregate worker metrics for a future admin health page (issue #953,
//! part of the "Admin Server Health" epic alongside #952/#954/#955): a
//! per-task-type queue depth plus a bounded window of recent completion
//! timings. Distinct from [`super::progress`]'s `progress_snapshot`, which
//! is a live per-task feed — this module summarizes across every
//! currently-tracked task by [`TaskKind`] and never mutates or evicts the
//! progress map's own state.

use std::collections::HashMap;
use std::time::Duration;

use omnibus_shared::TaskKind;

use super::types::{lock_unpoison, Worker};

/// Recent-completions window kept per [`TaskKind`]. Small and fixed so the
/// in-memory window stays bounded no matter how long the worker has been
/// running; 20 is enough for a health page to render a meaningful recent
/// spread (e.g. min/median/max) without unbounded bookkeeping cost.
pub(super) const RECENT_COMPLETIONS_CAP: usize = 20;

/// Aggregate, per-task-type snapshot of worker queue state: how many tasks
/// of each [`TaskKind`] are currently queued or running, and how long the
/// most recently completed tasks of each kind took. Returned by
/// [`Worker::metrics`]; unlike [`Worker::progress_snapshot`](super::Worker::progress_snapshot)
/// (a live per-task feed) this is a coarse summary meant for cheap,
/// periodic polling.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkerMetrics {
    /// Count of non-terminal (queued or running) tasks per [`TaskKind`].
    /// A kind with no live tasks is simply absent rather than mapped to 0.
    pub queue_depth: HashMap<TaskKind, usize>,
    /// Durations of the most recent completions per [`TaskKind`], oldest
    /// first, capped at [`RECENT_COMPLETIONS_CAP`] entries per kind.
    /// Recorded on every terminal transition (`Done` or `Failed`) — this
    /// reports how long tasks are taking, not whether they succeeded.
    pub recent_completions: HashMap<TaskKind, Vec<Duration>>,
}

impl Worker {
    /// Cheap, read-only snapshot of per-task-type queue depth and recent
    /// completion timings, for a future admin health page. Two short lock
    /// scopes, neither of which is the write path `progress_snapshot`
    /// relies on for eviction, so calling this never disturbs the live
    /// progress feed.
    pub fn metrics(&self) -> WorkerMetrics {
        let mut queue_depth: HashMap<TaskKind, usize> = HashMap::new();
        for entry in lock_unpoison(&self.progress).values() {
            if entry.terminal_at.is_none() {
                *queue_depth.entry(entry.progress.kind).or_insert(0) += 1;
            }
        }

        let recent_completions = lock_unpoison(&self.completion_timings)
            .iter()
            .map(|(kind, timings)| (*kind, timings.iter().copied().collect()))
            .collect();

        WorkerMetrics {
            queue_depth,
            recent_completions,
        }
    }

    /// Record one task's completion duration into its [`TaskKind`]'s
    /// bounded recent-completions window, evicting the oldest entry once
    /// [`RECENT_COMPLETIONS_CAP`] is exceeded. Called from
    /// [`super::progress::write_terminal_progress`] so every terminal
    /// transition counts as one completion.
    pub(super) fn record_completion(&self, kind: TaskKind, duration: Duration) {
        let mut map = lock_unpoison(&self.completion_timings);
        let window = map.entry(kind).or_default();
        window.push_back(duration);
        if window.len() > RECENT_COMPLETIONS_CAP {
            window.pop_front();
        }
    }
}
