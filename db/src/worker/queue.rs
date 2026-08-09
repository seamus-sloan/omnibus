//! Posting and awaiting tasks: `Worker::post`, `await_completion`, and
//! the RAII guards that keep the `completions` / `progress` maps bounded
//! even when a spawned future unwinds.
//!
//! `prune_resource_lock` lives here too because it's the
//! map-bookkeeping companion to the keyed-mutex acquire in
//! [`super::exec`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex};

use omnibus_shared::{ProgressState, TaskProgress};
use tokio::sync::watch;

use super::types::{
    lock_unpoison, wall_clock_ms, ProgressEntry, Task, TaskId, TaskOutcome, TaskSuccessDetail,
    Worker,
};

/// RAII guard that reclaims a `Worker::completions` slot when dropped.
/// Used inside [`Worker::post`]'s spawned future so the slot is removed
/// regardless of whether the future returned normally or unwound through
/// a panic — keeping the map bounded on the panic path as well as the
/// happy path.
struct CompletionsPruneGuard {
    completions: Arc<StdMutex<HashMap<TaskId, watch::Receiver<Option<TaskOutcome>>>>>,
    id: TaskId,
}

impl Drop for CompletionsPruneGuard {
    fn drop(&mut self) {
        // Recover on poison via `lock_unpoison` so the slot is reclaimed even
        // after another task panicked while holding this lock — otherwise the
        // map would grow unbounded. `into_inner` never panics, so this stays
        // safe on the unwinding path (no double-panic).
        lock_unpoison(&self.completions).remove(&self.id);
    }
}

/// RAII guard that records a terminal `Failed { "task panicked" }`
/// progress entry if the spawned future unwinds before [`Worker::run`]
/// writes one itself. Mirrors [`CompletionsPruneGuard`]'s shape — the
/// "happy path completed first" check is the `terminal_at.is_some()`
/// inspection so a clean run leaves the existing terminal alone.
struct ProgressTerminalGuard {
    progress: Arc<StdMutex<BTreeMap<TaskId, ProgressEntry>>>,
    id: TaskId,
}

impl Drop for ProgressTerminalGuard {
    fn drop(&mut self) {
        // Recover on poison via `lock_unpoison` rather than skipping on `Err`:
        // a poisoned `progress` map would otherwise leave this task's entry
        // stuck in `Running` forever (never evicted, UI shows a stuck task),
        // so the terminal "task panicked" write is exactly what must still
        // happen. `into_inner` never panics, so there is no double-panic risk
        // on the unwinding path.
        let mut map = lock_unpoison(&self.progress);
        if let Some(entry) = map.get_mut(&self.id) {
            if entry.terminal_at.is_none() {
                let now_ms = wall_clock_ms();
                entry.progress.state = ProgressState::Failed {
                    message: "task panicked".to_string(),
                };
                entry.progress.last_update_ms = now_ms;
                entry.terminal_at = Some(std::time::Instant::now());
            }
        }
    }
}

impl Worker {
    /// Spawn `task` and return its [`TaskId`] immediately, without waiting
    /// for it to run. The id can later be passed to
    /// [`await_completion`](Worker::await_completion) to retrieve the
    /// [`TaskOutcome`]. Scheduling (resource lock + scan semaphore) and
    /// execution happen inside the spawned future, so a posted task may
    /// queue behind same-resource or scan-capped peers before running. A
    /// failed task is logged via `tracing` in addition to being reported to
    /// awaiters.
    pub fn post(self: &Arc<Self>, task: Task) -> TaskId {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Store the receiver, not the sender: if the spawned task panics
        // before sending, dropping `tx` closes the channel and any pending
        // `await_completion` falls through to the "dropped" error branch.
        let (tx, rx) = watch::channel(None);
        lock_unpoison(&self.completions).insert(id, rx);

        // Seed the progress map *before* spawning so a caller polling
        // `progress_snapshot()` immediately after `post` returns always
        // observes the task. The entry stays `Running { 0, None }` while
        // the task queues behind the resource lock or scan semaphore,
        // which is exactly the "queued behind another scan" state the UI
        // wants to surface.
        {
            let now_ms = wall_clock_ms();
            let entry = ProgressEntry {
                progress: TaskProgress {
                    task_id: id,
                    kind: task.kind(),
                    state: ProgressState::Running {
                        processed: 0,
                        total: None,
                    },
                    resource_key: task.resource_key(),
                    started_at_ms: now_ms,
                    last_update_ms: now_ms,
                },
                terminal_at: None,
                owner: None,
            };
            lock_unpoison(&self.progress).insert(id, entry);
        }

        let this = self.clone();
        let completions = self.completions.clone();
        let progress = self.progress.clone();
        tokio::spawn(async move {
            // RAII guard so the slot is reclaimed on the normal happy path
            // *and* on unwind from a panic inside `run`. Without this, a
            // panicking handler would leave its slot in the map forever —
            // one leaked entry per panic for fire-and-forget posts (the
            // boot / settings-save reindex kicks and the per-author photo
            // resolutions), which is exactly the unbounded growth this
            // refactor exists to prevent.
            let _prune = CompletionsPruneGuard { completions, id };
            // Sister guard for the progress map: if `run` unwinds before
            // recording its own terminal state, this writes a synthetic
            // `Failed { "task panicked" }` so the UI's red-banner path
            // fires the same way it does for an `Err(_)` outcome. On the
            // happy path `run` records the real terminal and this drop is
            // a no-op (terminal_at is already Some). Declared *after*
            // `_prune` so it drops *before* it: `retained_outcome` relies on
            // a terminal state existing by the time the slot disappears.
            let _progress_guard = ProgressTerminalGuard {
                progress: progress.clone(),
                id,
            };

            let outcome = this.run(task, id).await;
            if let TaskOutcome::Err(ref msg) = outcome {
                tracing::error!(
                    task_id = id,
                    error = %msg,
                    "worker: task failed"
                );
            }
            // Publish the terminal outcome, then let `_prune` drop the map
            // slot. A `watch::Receiver` that `await_completion` took out of
            // the map *before* this runs keeps observing the final value
            // even after `tx` drops and the slot is gone, so an in-flight
            // awaiter still resolves; one that arrives *after* the slot is
            // gone reads the retained progress entry instead.
            let _ = tx.send(Some(outcome));
        });

        id
    }

    /// Wait for the task identified by `id` to finish and return its
    /// [`TaskOutcome`]. A task that already finished still reports its real
    /// outcome for as long as the progress map retains the terminal entry
    /// (~[`super::types::TERMINAL_RETENTION`]). Returns
    /// `TaskOutcome::Err("unknown task id")` once that window has passed, or
    /// if `id` was never posted; [`TaskOutcome::Err`] likewise if the spawned
    /// task was dropped before reporting an outcome.
    pub async fn await_completion(&self, id: TaskId) -> TaskOutcome {
        // Take ownership of the receiver out of the map rather than cloning
        // it. The held receiver observes the channel's final value regardless
        // of whether the spawned task has already dropped its sender, so the
        // outcome is never missed; removing the slot here bounds the map even
        // when the run loop's own cleanup hasn't fired yet.
        let mut rx = {
            let mut map = lock_unpoison(&self.completions);
            match map.remove(&id) {
                Some(rx) => rx,
                // No slot: most often the task finished first and
                // `CompletionsPruneGuard` got here before the caller did.
                None => return self.retained_outcome(id),
            }
        };
        loop {
            if let Some(outcome) = rx.borrow().clone() {
                return outcome;
            }
            if rx.changed().await.is_err() {
                return TaskOutcome::Err("worker dropped task before completion".into());
            }
        }
    }

    /// Recover a finished task's outcome from its retained progress entry,
    /// for a caller that reached [`await_completion`](Worker::await_completion)
    /// after the completion slot was already reclaimed.
    ///
    /// The progress map is the right place to read this from rather than a
    /// second retention window on `completions`: it already retains terminal
    /// entries for [`super::types::TERMINAL_RETENTION`] and already evicts
    /// them, so nothing new can grow unbounded. The read is sound because
    /// `post` drops [`ProgressTerminalGuard`] *before*
    /// [`CompletionsPruneGuard`] — so whenever the slot is gone, the terminal
    /// state has already been written.
    fn retained_outcome(&self, id: TaskId) -> TaskOutcome {
        let terminal = lock_unpoison(&self.progress)
            .get(&id)
            .filter(|entry| entry.terminal_at.is_some())
            .map(|entry| entry.progress.state.clone());
        match terminal {
            // Inverse of `exec::project_terminal`: a task attaches at most one
            // detail, so at most one of these two fields is ever set.
            Some(ProgressState::Done {
                ghost_warning,
                bake_errors,
                ..
            }) => TaskOutcome::Ok(
                ghost_warning
                    .map(TaskSuccessDetail::GhostFiles)
                    .or_else(|| bake_errors.map(TaskSuccessDetail::BakeErrors)),
            ),
            Some(ProgressState::Failed { message }) => TaskOutcome::Err(message),
            // Never posted, evicted after the retention window, or a
            // concurrent awaiter already took the receiver while the task is
            // still running — none of which this handle can resolve.
            _ => TaskOutcome::Err("unknown task id".into()),
        }
    }

    /// Non-blocking peek at a task's current lifecycle state, read from the
    /// progress map by id. Returns `None` once `id` was never posted or its
    /// entry has been evicted (~[`super::types::TERMINAL_RETENTION`] after the
    /// task reached a terminal state). Unlike [`await_completion`](Worker::await_completion)
    /// this neither blocks nor consumes anything, so an enqueue-and-poll caller
    /// can call it repeatedly: post once, then poll this until it observes a
    /// terminal [`ProgressState`].
    pub fn task_state(&self, id: TaskId) -> Option<ProgressState> {
        lock_unpoison(&self.progress)
            .get(&id)
            .map(|e| e.progress.state.clone())
    }

    /// Record that `user_id` owns the task `id`. Call right after
    /// [`post`](Worker::post) for user-initiated, pollable jobs (e.g.
    /// Send-to-Kindle) so [`owned_task_state`](Worker::owned_task_state) can
    /// scope status reads to the owner. A no-op if the entry was already
    /// evicted (it won't have been this soon after `post`).
    pub fn set_task_owner(&self, id: TaskId, user_id: i64) {
        if let Some(entry) = lock_unpoison(&self.progress).get_mut(&id) {
            entry.owner = Some(user_id);
        }
    }

    /// Owner-scoped [`task_state`](Worker::task_state): returns the state only
    /// when `user_id` matches the task's recorded owner. Returns `None` for
    /// unknown, evicted, unowned, or other-user tasks — so an authenticated
    /// caller can't probe the guessable, monotonic task-id space to read
    /// another user's send outcome (which can carry an SMTP error message).
    pub fn owned_task_state(&self, id: TaskId, user_id: i64) -> Option<ProgressState> {
        lock_unpoison(&self.progress)
            .get(&id)
            .filter(|e| e.owner == Some(user_id))
            .map(|e| e.progress.state.clone())
    }

    /// Reclaim a keyed resource mutex once no other task references it. Held
    /// under the `resource_locks` map lock so a concurrent `run` can't be
    /// mid-`entry()` for the same key: the map's own `Arc` plus any live
    /// runner (each holds a clone before/while awaiting the keyed mutex) each
    /// count as one strong reference, so a count of 1 — only the map — means
    /// the slot is free to drop. A later task for the same key just
    /// re-inserts a fresh mutex via `or_insert_with`.
    pub(super) async fn prune_resource_lock(&self, key: &str) {
        let mut map = self.resource_locks.lock().await;
        if let Some(inner) = map.get(key) {
            if Arc::strong_count(inner) == 1 {
                map.remove(key);
            }
        }
    }
}
