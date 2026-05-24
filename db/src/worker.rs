//! Background-worker primitive (F0.5).
//!
//! Single-process queue with two fairness knobs:
//! - `scan_concurrency` caps how many `Task::Scan` jobs run concurrently
//!   (acquired from a per-Worker [`Semaphore`]).
//! - A per-resource keyed mutex map serializes any tasks that share the
//!   same resource key, so e.g. two scans of the same library path queue
//!   behind each other while different paths run in parallel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use sqlx::SqlitePool;
use tokio::sync::{watch, Mutex, Semaphore};

/// A unit of background work handed to [`Worker::post`].
///
/// Each variant carries the inputs its handler needs and determines two
/// scheduling properties via the private `resource_key` / `uses_scan_sem`
/// helpers: which per-resource keyed mutex (if any) serializes it against
/// peers, and whether it counts against the scan-concurrency semaphore.
/// See [`Worker`] for how those interact.
///
/// `#[non_exhaustive]` so adding a variant is not a breaking change for
/// downstream `match`es; new variants must wire up both scheduling helpers
/// and the `execute` dispatch arm.
#[non_exhaustive]
pub enum Task {
    /// Reindex the library rooted at `library_path` (full scan → DB upsert
    /// via `indexer::reindex`). Keyed on the path, so two scans of the same
    /// library serialize while different libraries scan in parallel; counts
    /// against the scan-concurrency semaphore.
    Scan { library_path: String },
    /// (Re)generate cached WebP thumbnails for `book_id`'s cover.
    /// `last_modified_epoch` lets the handler skip work when the cached
    /// thumbnails are already current. Keyed on `thumb:{book_id}` and does
    /// not consume the scan semaphore, so thumbnailing runs alongside scans.
    GenerateThumbs {
        book_id: i64,
        last_modified_epoch: i64,
    },
    /// F1.11: resolve and cache an author's profile photo. The resolver
    /// hits Open Library at most once per author per (admin-DELETE-able)
    /// cache window; a `'letter'` marker is written on any miss so future
    /// page views skip the network entirely. Keyed on
    /// `author-photo:{author_id}` and does not consume the scan semaphore.
    ResolveAuthorPhoto { author_id: i64 },
    /// Test-only synthetic task: sleeps `latency_ms` and invokes the
    /// optional `on_run` / `on_done` hooks, with `resource` and
    /// `route_through_scan_sem` letting a test exercise the keyed mutex and
    /// scan semaphore directly. Compiled out of non-test builds.
    #[cfg(test)]
    Test {
        tag: &'static str,
        latency_ms: u64,
        resource: Option<String>,
        route_through_scan_sem: bool,
        on_run: Option<Arc<dyn Fn() + Send + Sync>>,
        on_done: Option<Arc<dyn Fn() + Send + Sync>>,
    },
}

impl Task {
    fn resource_key(&self) -> Option<String> {
        match self {
            Task::Scan { library_path } => Some(library_path.clone()),
            Task::GenerateThumbs { book_id, .. } => Some(format!("thumb:{book_id}")),
            Task::ResolveAuthorPhoto { author_id } => Some(format!("author-photo:{author_id}")),
            #[cfg(test)]
            Task::Test { resource, .. } => resource.clone(),
        }
    }

    fn uses_scan_sem(&self) -> bool {
        match self {
            Task::Scan { .. } => true,
            Task::GenerateThumbs { .. } => false,
            Task::ResolveAuthorPhoto { .. } => false,
            #[cfg(test)]
            Task::Test {
                route_through_scan_sem,
                ..
            } => *route_through_scan_sem,
        }
    }
}

/// Process-local handle returned by [`Worker::post`], used to look up a
/// task's completion via [`Worker::await_completion`]. Monotonically
/// assigned per `Worker`; not stable across restarts and not a DB id.
pub type TaskId = u64;

/// Terminal result of a task, delivered to awaiters of its [`TaskId`].
#[derive(Clone, Debug)]
pub enum TaskOutcome {
    /// The handler ran to completion successfully.
    Ok,
    /// The handler failed; the string is the stringified underlying error.
    /// Also produced when the spawned task is dropped or panics before
    /// reporting (see [`Worker::await_completion`]).
    Err(String),
}

/// Construction-time tuning for a [`Worker`].
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Maximum number of scan-semaphore tasks (currently [`Task::Scan`])
    /// allowed to run concurrently. Clamped to at least 1 by
    /// [`Worker::new`]. Other task types are unaffected by this cap.
    pub scan_concurrency: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            scan_concurrency: 1,
        }
    }
}

/// Single-process background-task runner shared behind an `Arc`.
///
/// Posting a [`Task`] spawns it on the tokio runtime and returns a
/// [`TaskId`] immediately ([`post`](Worker::post) returns as soon as the
/// task is spawned; it does not wait for the task to run to completion).
/// Two fairness mechanisms shape execution:
///
/// * **Per-resource keyed mutex** — tasks reporting the same resource key
///   (e.g. two scans of the same library path) serialize behind one
///   another, while tasks on different keys run concurrently.
/// * **Scan-concurrency semaphore** — scan-class tasks additionally
///   contend for a fixed pool of permits sized by
///   [`WorkerConfig::scan_concurrency`], capping how many run at once
///   regardless of resource key.
///
/// The resource lock is always acquired before the scan permit so a task
/// queued behind a same-resource peer never holds a permit while idle.
/// Owns the [`SqlitePool`] its handlers run against.
pub struct Worker {
    pool: SqlitePool,
    scan_sem: Arc<Semaphore>,
    resource_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    completions: Arc<StdMutex<HashMap<TaskId, watch::Receiver<Option<TaskOutcome>>>>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl Worker {
    /// Build a `Worker` over `pool` with the given `config`, returning it
    /// behind an `Arc` (every method takes `&Arc<Self>` or `&self`, and
    /// posted tasks clone the `Arc` into their spawned future).
    pub fn new(pool: SqlitePool, config: WorkerConfig) -> Arc<Self> {
        Arc::new(Self {
            pool,
            scan_sem: Arc::new(Semaphore::new(config.scan_concurrency.max(1))),
            resource_locks: Arc::new(Mutex::new(HashMap::new())),
            completions: Arc::new(StdMutex::new(HashMap::new())),
            next_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    #[cfg(test)]
    fn completions_len(&self) -> usize {
        self.completions.lock().unwrap().len()
    }

    #[cfg(test)]
    async fn resource_locks_len(&self) -> usize {
        self.resource_locks.lock().await.len()
    }

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
        self.completions.lock().unwrap().insert(id, rx);

        let this = self.clone();
        tokio::spawn(async move {
            let outcome = this.run(task).await;
            if let TaskOutcome::Err(ref msg) = outcome {
                tracing::error!(
                    task_id = id,
                    error = %msg,
                    "worker: task failed"
                );
            }
            // Publish the terminal outcome, then drop the map slot. A
            // `watch::Receiver` that `await_completion` took out of the map
            // *before* this runs keeps observing the final value even after
            // `tx` drops and the slot is gone, so an in-flight awaiter still
            // resolves. Cleaning up here (not only in `await_completion`) is
            // what bounds the map for the fire-and-forget post paths that
            // never await — e.g. the boot-time and settings-save reindex
            // kicks, which post a `Task::Scan` and discard the id. Without
            // this the map grew by one entry per dispatched task for the
            // process lifetime.
            let _ = tx.send(Some(outcome));
            this.completions.lock().unwrap().remove(&id);
        });

        id
    }

    /// Wait for the task identified by `id` to finish and return its
    /// [`TaskOutcome`]. Returns [`TaskOutcome::Err`] if `id` was never posted
    /// (or already pruned) or if the spawned task was dropped before reporting
    /// an outcome (e.g. it panicked, which closes the watch channel).
    pub async fn await_completion(&self, id: TaskId) -> TaskOutcome {
        // Take ownership of the receiver out of the map rather than cloning
        // it. The held receiver observes the channel's final value regardless
        // of whether the spawned task has already dropped its sender, so the
        // outcome is never missed; removing the slot here bounds the map even
        // when the run loop's own cleanup hasn't fired yet.
        let mut rx = {
            let mut map = self.completions.lock().unwrap();
            match map.remove(&id) {
                Some(rx) => rx,
                None => return TaskOutcome::Err("unknown task id".into()),
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

    async fn run(self: &Arc<Self>, task: Task) -> TaskOutcome {
        // Resource lock first, then the scan semaphore: holding a permit
        // while blocked on a per-resource mutex would let same-resource
        // queueing starve other resources from running concurrently.
        let resource_key = task.resource_key();
        let _resource_guard = if let Some(key) = resource_key.clone() {
            let inner = {
                let mut map = self.resource_locks.lock().await;
                map.entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            };
            Some(inner.lock_owned().await)
        } else {
            None
        };

        let _scan_permit = if task.uses_scan_sem() {
            match self.scan_sem.clone().acquire_owned().await {
                Ok(p) => Some(p),
                Err(_) => return TaskOutcome::Err("scan semaphore closed".into()),
            }
        } else {
            None
        };

        let outcome = self.execute(task).await;

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

    /// Reclaim a keyed resource mutex once no other task references it. Held
    /// under the `resource_locks` map lock so a concurrent `run` can't be
    /// mid-`entry()` for the same key: the map's own `Arc` plus any live
    /// runner (each holds a clone before/while awaiting the keyed mutex) each
    /// count as one strong reference, so a count of 1 — only the map — means
    /// the slot is free to drop. A later task for the same key just
    /// re-inserts a fresh mutex via `or_insert_with`.
    async fn prune_resource_lock(&self, key: &str) {
        let mut map = self.resource_locks.lock().await;
        if let Some(inner) = map.get(key) {
            if Arc::strong_count(inner) == 1 {
                map.remove(key);
            }
        }
    }

    async fn execute(&self, task: Task) -> TaskOutcome {
        match task {
            Task::Scan { library_path } => {
                match crate::indexer::reindex(&self.pool, &library_path).await {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::ResolveAuthorPhoto { author_id } => {
                match crate::author_photos::resolve(&self.pool, author_id).await {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::GenerateThumbs {
                book_id,
                last_modified_epoch,
            } => {
                let pool = self.pool.clone();
                let cover = match crate::queries::get_cover(&pool, book_id).await {
                    Ok(Some((_mime, bytes))) => bytes,
                    Ok(None) => {
                        return TaskOutcome::Err(format!("no cover for book {book_id}"));
                    }
                    Err(e) => return TaskOutcome::Err(e.to_string()),
                };
                let cap = crate::thumbs::cap_bytes();
                match tokio::task::spawn_blocking(move || {
                    crate::thumbs::ensure_thumbnails_sync(book_id, last_modified_epoch, cover)?;
                    crate::thumbs::evict_if_over_cap(cap)
                        .map_err(|e| crate::thumbs::ThumbError::Io(e.to_string()))
                })
                .await
                {
                    Ok(Ok(())) => TaskOutcome::Ok,
                    Ok(Err(e)) => TaskOutcome::Err(e.to_string()),
                    Err(join_err) => {
                        // `JoinError` covers both panics and cancellation —
                        // distinguish so the log doesn't lie about which one.
                        let kind = if join_err.is_panic() {
                            "panicked"
                        } else {
                            "was cancelled"
                        };
                        TaskOutcome::Err(format!("spawn_blocking {kind}: {join_err}"))
                    }
                }
            }
            #[cfg(test)]
            Task::Test {
                tag: _,
                latency_ms,
                on_run,
                on_done,
                ..
            } => {
                if let Some(f) = on_run.as_ref() {
                    f();
                }
                tokio::time::sleep(std::time::Duration::from_millis(latency_ms)).await;
                if let Some(f) = on_done.as_ref() {
                    f();
                }
                TaskOutcome::Ok
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Mutex as TestMutex;
    use std::time::Instant;

    async fn pool() -> SqlitePool {
        crate::init_db("sqlite::memory:").await.unwrap()
    }

    fn make_worker_default(pool: SqlitePool) -> Arc<Worker> {
        Worker::new(pool, WorkerConfig::default())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn post_runs_a_task() {
        let w = make_worker_default(pool().await);
        let id = w.post(Task::Test {
            tag: "basic",
            latency_ms: 10,
            resource: None,
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        match w.await_completion(id).await {
            TaskOutcome::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn same_resource_serializes() {
        let w = make_worker_default(pool().await);
        let intervals: Arc<TestMutex<Vec<(Instant, Instant)>>> =
            Arc::new(TestMutex::new(Vec::new()));

        let mk = |w: &Arc<Worker>, intervals: Arc<TestMutex<Vec<(Instant, Instant)>>>| {
            let starts = Arc::new(std::sync::Mutex::new(None::<Instant>));
            let starts_run = starts.clone();
            let intervals_done = intervals.clone();
            let starts_done = starts.clone();
            w.post(Task::Test {
                tag: "k",
                latency_ms: 80,
                resource: Some("k".into()),
                route_through_scan_sem: false,
                on_run: Some(Arc::new(move || {
                    *starts_run.lock().unwrap() = Some(Instant::now());
                })),
                on_done: Some(Arc::new(move || {
                    let start = starts_done.lock().unwrap().expect("on_run before on_done");
                    let end = Instant::now();
                    intervals_done.lock().unwrap().push((start, end));
                })),
            })
        };

        let id1 = mk(&w, intervals.clone());
        let id2 = mk(&w, intervals.clone());

        let _ = tokio::join!(w.await_completion(id1), w.await_completion(id2));

        let mut ivs = intervals.lock().unwrap().clone();
        ivs.sort_by_key(|(s, _)| *s);
        assert_eq!(ivs.len(), 2);
        assert!(
            ivs[0].1 <= ivs[1].0,
            "expected serialized intervals, got {:?}",
            ivs
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn different_resources_run_in_parallel() {
        let w = make_worker_default(pool().await);
        let intervals: Arc<TestMutex<Vec<(Instant, Instant)>>> =
            Arc::new(TestMutex::new(Vec::new()));

        let mk = |w: &Arc<Worker>,
                  key: &'static str,
                  intervals: Arc<TestMutex<Vec<(Instant, Instant)>>>| {
            let starts = Arc::new(std::sync::Mutex::new(None::<Instant>));
            let starts_run = starts.clone();
            let intervals_done = intervals.clone();
            let starts_done = starts.clone();
            w.post(Task::Test {
                tag: key,
                latency_ms: 80,
                resource: Some(key.into()),
                route_through_scan_sem: false,
                on_run: Some(Arc::new(move || {
                    *starts_run.lock().unwrap() = Some(Instant::now());
                })),
                on_done: Some(Arc::new(move || {
                    let start = starts_done.lock().unwrap().expect("on_run before on_done");
                    let end = Instant::now();
                    intervals_done.lock().unwrap().push((start, end));
                })),
            })
        };

        let id1 = mk(&w, "a", intervals.clone());
        let id2 = mk(&w, "b", intervals.clone());

        let _ = tokio::join!(w.await_completion(id1), w.await_completion(id2));

        let mut ivs = intervals.lock().unwrap().clone();
        ivs.sort_by_key(|(s, _)| *s);
        assert_eq!(ivs.len(), 2);
        assert!(
            ivs[0].1 > ivs[1].0,
            "expected overlapping intervals, got {:?}",
            ivs
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrency_cap_respected() {
        let w = Worker::new(
            pool().await,
            WorkerConfig {
                scan_concurrency: 1,
            },
        );
        let running = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mk = |w: &Arc<Worker>,
                  key: &'static str,
                  running: Arc<AtomicUsize>,
                  max_seen: Arc<AtomicUsize>| {
            let running_run = running.clone();
            let max_seen_run = max_seen.clone();
            let running_done = running.clone();
            w.post(Task::Test {
                tag: key,
                latency_ms: 50,
                resource: Some(key.into()),
                route_through_scan_sem: true,
                on_run: Some(Arc::new(move || {
                    let n = running_run.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                    max_seen_run.fetch_max(n, AtomicOrdering::SeqCst);
                })),
                on_done: Some(Arc::new(move || {
                    running_done.fetch_sub(1, AtomicOrdering::SeqCst);
                })),
            })
        };

        let id1 = mk(&w, "a", running.clone(), max_seen.clone());
        let id2 = mk(&w, "b", running.clone(), max_seen.clone());
        let id3 = mk(&w, "c", running.clone(), max_seen.clone());

        let _ = tokio::join!(
            w.await_completion(id1),
            w.await_completion(id2),
            w.await_completion(id3),
        );

        assert_eq!(
            max_seen.load(AtomicOrdering::SeqCst),
            1,
            "scan_concurrency=1 should never observe >1 running"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn await_completion_unknown_id_errors() {
        let w = make_worker_default(pool().await);
        match w.await_completion(99999).await {
            TaskOutcome::Err(_) => {}
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn await_completion_returns_err_when_task_panics() {
        let w = make_worker_default(pool().await);
        let id = w.post(Task::Test {
            tag: "panicker",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: Some(Arc::new(|| panic!("intentional test panic"))),
            on_done: None,
        });
        match w.await_completion(id).await {
            TaskOutcome::Err(_) => {}
            other => panic!("expected Err on task panic, got {other:?}"),
        }
    }

    /// Poll until both worker maps are empty or a deadline elapses, so
    /// fire-and-forget assertions don't hinge on a fixed sleep. Returns
    /// whether the maps drained in time.
    async fn poll_maps_empty(w: &Arc<Worker>) -> bool {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if w.completions_len() == 0 && w.resource_locks_len().await == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Like [`poll_maps_empty`] but waits only on `resource_locks`.
    async fn poll_resource_locks_empty(w: &Arc<Worker>) -> bool {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if w.resource_locks_len().await == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn await_completion_prunes_completions_entry() {
        let w = make_worker_default(pool().await);
        for _ in 0..5 {
            let id = w.post(Task::Test {
                tag: "prune",
                latency_ms: 5,
                resource: None,
                route_through_scan_sem: false,
                on_run: None,
                on_done: None,
            });
            let _ = w.await_completion(id).await;
        }
        // Each awaited task removes its own slot before returning, so the map
        // stays bounded no matter how many tasks have run.
        assert_eq!(
            w.completions_len(),
            0,
            "completions map should be empty after awaiting every task"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fire_and_forget_tasks_drain_both_maps() {
        // Mirrors the boot / settings-save reindex kicks (server::main,
        // rpc::save_settings): post and discard the id, never awaiting. The
        // run loop must still reclaim both map slots.
        let w = make_worker_default(pool().await);
        for i in 0..5 {
            // Distinct resource keys exercise the `resource_locks` prune path.
            w.post(Task::Test {
                tag: "ff",
                latency_ms: 5,
                resource: Some(format!("k{i}")),
                route_through_scan_sem: false,
                on_run: None,
                on_done: None,
            });
        }
        let drained = poll_maps_empty(&w).await;
        assert!(
            drained,
            "fire-and-forget tasks must drain completions ({}) and resource_locks ({})",
            w.completions_len(),
            w.resource_locks_len().await,
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn same_resource_serialized_tasks_leave_no_lock_behind() {
        // Two tasks share a key, so the second clones the keyed mutex `Arc`
        // and waits while the first runs. The prune must not pull the lock
        // out from under the waiter, and once both finish the slot is gone.
        let w = make_worker_default(pool().await);
        let mk = |w: &Arc<Worker>, tag: &'static str| {
            w.post(Task::Test {
                tag,
                latency_ms: 30,
                resource: Some("shared".into()),
                route_through_scan_sem: false,
                on_run: None,
                on_done: None,
            })
        };
        let id1 = mk(&w, "s1");
        let id2 = mk(&w, "s2");
        let (o1, o2) = tokio::join!(w.await_completion(id1), w.await_completion(id2));
        assert!(matches!(o1, TaskOutcome::Ok));
        assert!(matches!(o2, TaskOutcome::Ok));
        // The run loop prunes after dropping its guard; allow the second
        // task's cleanup to land after its outcome was observed.
        let drained = poll_resource_locks_empty(&w).await;
        assert!(
            drained,
            "shared resource lock should be reclaimed once both tasks finish, got {}",
            w.resource_locks_len().await
        );
        assert_eq!(w.completions_len(), 0);
    }
}
