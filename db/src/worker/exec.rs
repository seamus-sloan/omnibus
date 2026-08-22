//! The `Worker::run` dispatch loop: acquire the per-resource keyed mutex,
//! then the scan-concurrency permit, run the task via
//! [`super::handlers`]'s `execute`, project the outcome into a terminal
//! [`ProgressState`], and drop both guards before pruning the keyed
//! mutex.

use std::sync::Arc;

use omnibus_shared::ProgressState;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::types::{lock_unpoison, Task, TaskId, TaskOutcome, TaskSuccessDetail, Worker};

impl Worker {
    pub(super) async fn run(self: &Arc<Self>, task: Task, id: TaskId) -> TaskOutcome {
        // Resource lock first, then the scan semaphore: holding a permit
        // while blocked on a per-resource mutex would let same-resource
        // queueing starve other resources from running concurrently.
        let resource_key = task.resource_key();
        let _resource_guard = if let Some(key) = resource_key.clone() {
            let inner = {
                let mut map = self.resource_locks.lock().await;
                map.entry(key)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                    .clone()
            };
            Some(inner.lock_owned().await)
        } else {
            None
        };

        let _scan_permit = if task.uses_scan_sem() {
            match self
                .acquire_permit(&self.scan_sem, id, "scan semaphore closed")
                .await
            {
                Ok(p) => Some(p),
                Err(outcome) => return outcome,
            }
        } else {
            None
        };

        let _hls_permit = if task.uses_hls_sem() {
            match self
                .acquire_permit(&self.hls_sem, id, "hls semaphore closed")
                .await
            {
                Ok(p) => Some(p),
                Err(outcome) => return outcome,
            }
        } else {
            None
        };

        let _convert_permit = if task.uses_convert_sem() {
            match self
                .acquire_permit(&self.convert_sem, id, "convert semaphore closed")
                .await
            {
                Ok(p) => Some(p),
                Err(outcome) => return outcome,
            }
        } else {
            None
        };

        let outcome = self.execute(task, id).await;
        self.write_terminal_progress(id, self.project_terminal(id, &outcome));

        // Drop the resource guard before pruning so this task no longer
        // counts as a reference to the keyed mutex when we check it. Without
        // this the map would grow by one `Arc<Mutex<()>>` per distinct
        // resource key for the process lifetime (thumbnails key per-book,
        // author photos per-author, scans per-path) — the same
        // unbounded-growth class as the completions map.
        drop(_resource_guard);
        if let Some(key) = resource_key {
            self.prune_resource_lock(&key).await;
        }

        outcome
    }

    /// Acquire one owned permit from `sem`. On a closed semaphore, write the
    /// terminal `Failed` progress with `closed_msg` and hand the caller the
    /// matching `Err(TaskOutcome)` to return from [`run`].
    async fn acquire_permit(
        &self,
        sem: &Arc<Semaphore>,
        id: TaskId,
        closed_msg: &str,
    ) -> Result<OwnedSemaphorePermit, TaskOutcome> {
        match sem.clone().acquire_owned().await {
            Ok(p) => Ok(p),
            Err(_) => {
                self.write_terminal_progress(
                    id,
                    ProgressState::Failed {
                        message: closed_msg.into(),
                    },
                );
                Err(TaskOutcome::Err(closed_msg.into()))
            }
        }
    }

    /// Project a task's `outcome` into its wire-facing terminal state. On
    /// success, the last reported `processed` count is pulled out of the
    /// progress map so a Phase-2 in-flight report stays reflected in the final
    /// `Done` (today there is no in-flight reporter, so this is always 0).
    /// The `TaskSuccessDetail` (a scan's ghost warning, or a bake's
    /// errors) rides straight through onto the matching `Done` field — a
    /// task produces at most one of the two, so the other is always `None`.
    fn project_terminal(&self, id: TaskId, outcome: &TaskOutcome) -> ProgressState {
        match outcome {
            TaskOutcome::Ok(detail) => {
                let processed = lock_unpoison(&self.progress)
                    .get(&id)
                    .and_then(|e| match e.progress.state {
                        ProgressState::Running { processed, .. } => Some(processed),
                        _ => None,
                    })
                    .unwrap_or(0);
                let (ghost_warning, bake_errors) = match detail {
                    Some(TaskSuccessDetail::GhostFiles(w)) => (Some(w.clone()), None),
                    Some(TaskSuccessDetail::BakeErrors(errors)) => (None, Some(errors.clone())),
                    None => (None, None),
                };
                ProgressState::Done {
                    processed,
                    ghost_warning,
                    bake_errors,
                }
            }
            TaskOutcome::Err(msg) => ProgressState::Failed {
                message: msg.clone(),
            },
        }
    }
}
