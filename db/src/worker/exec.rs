//! The `Worker::run` dispatch loop: acquire the per-resource keyed mutex,
//! then the scan-concurrency permit, run the task via
//! [`super::handlers`]'s `execute`, project the outcome into a terminal
//! [`ProgressState`], and drop both guards before pruning the keyed
//! mutex.

use std::sync::Arc;

use omnibus_shared::ProgressState;

use super::types::{lock_unpoison, Task, TaskId, TaskOutcome, Worker};

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
            match self.scan_sem.clone().acquire_owned().await {
                Ok(p) => Some(p),
                Err(_) => {
                    self.write_terminal_progress(
                        id,
                        ProgressState::Failed {
                            message: "scan semaphore closed".into(),
                        },
                    );
                    return TaskOutcome::Err("scan semaphore closed".into());
                }
            }
        } else {
            None
        };

        let _hls_permit = if task.uses_hls_sem() {
            match self.hls_sem.clone().acquire_owned().await {
                Ok(p) => Some(p),
                Err(_) => {
                    self.write_terminal_progress(
                        id,
                        ProgressState::Failed {
                            message: "hls semaphore closed".into(),
                        },
                    );
                    return TaskOutcome::Err("hls semaphore closed".into());
                }
            }
        } else {
            None
        };

        let outcome = self.execute(task, id).await;
        // Project the outcome into the wire-facing terminal state. We pull
        // the last reported `processed` count out of the progress map so a
        // Phase-2 in-flight progress report stays reflected in the final
        // `Done`. Today there is no in-flight reporter so this is always 0.
        let terminal = match &outcome {
            TaskOutcome::Ok => {
                let processed = lock_unpoison(&self.progress)
                    .get(&id)
                    .and_then(|e| match e.progress.state {
                        ProgressState::Running { processed, .. } => Some(processed),
                        _ => None,
                    })
                    .unwrap_or(0);
                ProgressState::Done { processed }
            }
            TaskOutcome::Err(msg) => ProgressState::Failed {
                message: msg.clone(),
            },
        };
        self.write_terminal_progress(id, terminal);

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
}
