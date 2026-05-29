//! Background-worker primitive (F0.5).
//!
//! Single-process queue with two fairness knobs:
//! - `scan_concurrency` caps how many `Task::Scan` jobs run concurrently
//!   (acquired from a per-Worker [`Semaphore`]).
//! - A per-resource keyed mutex map serializes any tasks that share the
//!   same resource key, so e.g. two scans of the same library path queue
//!   behind each other while different paths run in parallel.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use omnibus_shared::{ProgressState, TaskKind, TaskProgress, WorkerStatus};
use sqlx::SqlitePool;
use tokio::sync::{watch, Mutex, Semaphore};

/// How long a terminal ([`ProgressState::Done`] / [`ProgressState::Failed`])
/// entry sticks around in [`Worker::progress_snapshot`] after its
/// `terminal_at` timestamp before lazy eviction drops it. Long enough that
/// a 1 Hz polling client always observes the transition; short enough that
/// the in-memory map stays bounded by current concurrency + a handful of
/// recently-finished tasks.
const TERMINAL_RETENTION: Duration = Duration::from_secs(10);

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

    /// Wire-protocol discriminant exposed to the UI via
    /// [`Worker::progress_snapshot`]. Tests deliberately collapse onto an
    /// existing variant so the [`TaskKind`] enum doesn't grow a `Test` arm
    /// that downstream UIs would have to render.
    fn kind(&self) -> TaskKind {
        match self {
            Task::Scan { .. } => TaskKind::Scan,
            Task::GenerateThumbs { .. } => TaskKind::GenerateThumbs,
            Task::ResolveAuthorPhoto { .. } => TaskKind::ResolveAuthorPhoto,
            #[cfg(test)]
            Task::Test { .. } => TaskKind::Scan,
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
    /// Live snapshot of every posted task's lifecycle state. Holds entries
    /// from `post` until ~[`TERMINAL_RETENTION`] after the task reaches a
    /// terminal state. Read path is [`Worker::progress_snapshot`]; write
    /// paths are `post` (initial Running) and `run` (terminal Done/Failed,
    /// plus the panic-safety guard).
    progress: Arc<StdMutex<BTreeMap<TaskId, ProgressEntry>>>,
    next_id: std::sync::atomic::AtomicU64,
}

/// Internal pairing of the wire-facing [`TaskProgress`] with a monotonic
/// `terminal_at` timestamp used purely for eviction. We intentionally don't
/// reuse `TaskProgress.last_update_ms` (wall-clock; can jump under NTP) for
/// expiry decisions — that field exists only so the UI can render elapsed
/// time.
struct ProgressEntry {
    progress: TaskProgress,
    terminal_at: Option<Instant>,
}

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
                entry.terminal_at = Some(Instant::now());
            }
        }
    }
}

/// Recover from a poisoned `std::sync::Mutex` instead of panicking.
///
/// The `completions` / `progress` maps are best-effort bookkeeping that
/// every `Task::Scan` (and every other posted task) touches on its hot
/// path. If a thread panics while holding one of these locks — e.g. a
/// `ProgressTerminalGuard::drop` unwinding mid-write — `std::sync::Mutex`
/// poisons permanently, and a plain `.lock().unwrap()` would then turn that
/// one-off panic into a process-wide crash on every subsequent task. The
/// guarded data is just in-memory maps with no invariant left broken by a
/// partial write, so taking the inner guard and carrying on is strictly
/// safer than tearing down the worker. Mirrors `frontend::data`'s
/// `unpoison` helper.
fn lock_unpoison<T>(m: &StdMutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Current wall-clock time in milliseconds since the UNIX epoch. Used only
/// for the `started_at_ms` / `last_update_ms` fields on [`TaskProgress`],
/// which the UI renders as elapsed-time hints. A backward NTP step would
/// produce a wonky elapsed-time display for one polling tick — well within
/// the UI's tolerance.
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
            progress: Arc::new(StdMutex::new(BTreeMap::new())),
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

    #[cfg(test)]
    fn progress_len(&self) -> usize {
        self.progress.lock().unwrap().len()
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
            // a no-op (terminal_at is already Some).
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
            // awaiter still resolves.
            let _ = tx.send(Some(outcome));
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
            let mut map = lock_unpoison(&self.completions);
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

    async fn run(self: &Arc<Self>, task: Task, id: TaskId) -> TaskOutcome {
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

        let outcome = self.execute(task).await;
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

    /// Phase-2 seam: write the in-flight progress count for `id`. The
    /// terminal state is written separately at the end of [`Worker::run`]
    /// so a mid-task report can't accidentally flip a task to `Done`.
    /// Currently unused — wired up when `indexer::reindex` learns to
    /// thread a progress callback through its per-EPUB loop.
    #[allow(dead_code)]
    pub(crate) fn report_progress(&self, id: TaskId, processed: u32, total: Option<u32>) {
        let mut map = lock_unpoison(&self.progress);
        if let Some(entry) = map.get_mut(&id) {
            if entry.terminal_at.is_some() {
                return; // race: terminal already recorded
            }
            entry.progress.state = ProgressState::Running { processed, total };
            entry.progress.last_update_ms = wall_clock_ms();
        }
    }

    /// Internal terminal write. Called from `run` (Ok/Err) and from the
    /// `ProgressTerminalGuard` (panic). The `terminal_at` Instant is
    /// monotonic so eviction is robust to wall-clock drift.
    fn write_terminal_progress(&self, id: TaskId, state: ProgressState) {
        let mut map = lock_unpoison(&self.progress);
        if let Some(entry) = map.get_mut(&id) {
            entry.progress.last_update_ms = wall_clock_ms();
            entry.progress.state = state;
            entry.terminal_at = Some(Instant::now());
        }
    }

    /// Snapshot of every live worker task — non-terminal entries go in
    /// `active`, terminal entries (`Done` / `Failed`) less than
    /// [`TERMINAL_RETENTION`] old go in `recent_complete`. Older terminal
    /// entries are evicted under the same lock. Both vecs are sorted by
    /// `task_id` so a polling client renders a stable list across ticks.
    ///
    /// Auth-gated at the RPC layer; safe to call from any handler that
    /// already has an `AuthUser`.
    pub fn progress_snapshot(&self) -> WorkerStatus {
        self.progress_snapshot_with_retention(TERMINAL_RETENTION)
    }

    /// Test-friendly variant of [`Worker::progress_snapshot`] that lets
    /// callers supply a custom retention window. Production code always
    /// uses [`TERMINAL_RETENTION`]; the unit-test suite passes a much
    /// shorter window so the eviction assertion doesn't have to sleep
    /// for the full 10 s of wall-clock time.
    fn progress_snapshot_with_retention(&self, retention: Duration) -> WorkerStatus {
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
                let cover = match crate::covers::get_cover(&pool, book_id).await {
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
            "expected serialized intervals, got {ivs:?}"
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
            "expected overlapping intervals, got {ivs:?}"
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
    async fn fire_and_forget_panicking_tasks_drain_completions() {
        // Without an RAII guard on the spawn future, a panic in `run`
        // skips past the in-line `remove(&id)` and the slot leaks. Post a
        // batch of panicking fire-and-forget tasks and assert the map
        // drains anyway.
        let w = make_worker_default(pool().await);
        for _ in 0..5 {
            w.post(Task::Test {
                tag: "ff-panic",
                latency_ms: 0,
                resource: None,
                route_through_scan_sem: false,
                on_run: Some(Arc::new(|| panic!("intentional test panic"))),
                on_done: None,
            });
        }
        let drained = poll_maps_empty(&w).await;
        assert!(
            drained,
            "panicking fire-and-forget must drain completions ({})",
            w.completions_len()
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

    /// `progress_snapshot` reports every active entry as `recent_complete`
    /// terminals are evicted lazily on read. Block the run loop with a
    /// scan-sem-routed task so the snapshot fires before `execute` returns.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queued_task_shows_as_running_immediately() {
        let w = make_worker_default(pool().await);
        let id = w.post(Task::Test {
            tag: "queued",
            latency_ms: 50,
            resource: Some("queued".into()),
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        // Snapshot before the task finishes: the entry is the initial Running.
        let snap = w.progress_snapshot();
        assert!(snap.recent_complete.is_empty(), "no terminals yet");
        let entry = snap
            .active
            .iter()
            .find(|p| p.task_id == id)
            .expect("active entry seeded by post()");
        assert!(matches!(
            entry.state,
            ProgressState::Running {
                processed: 0,
                total: None
            }
        ));
        assert_eq!(entry.kind, TaskKind::Scan); // Test variant maps to Scan
        let _ = w.await_completion(id).await;
    }

    /// Two concurrent posts both surface in the snapshot. Uses non-overlapping
    /// resource keys so neither queues behind the other.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_tasks_show_two_active() {
        let w = make_worker_default(pool().await);
        let mk = |key: &'static str| {
            w.post(Task::Test {
                tag: key,
                latency_ms: 80,
                resource: Some(key.into()),
                route_through_scan_sem: false,
                on_run: None,
                on_done: None,
            })
        };
        let id1 = mk("aa");
        let id2 = mk("bb");

        // Briefly wait for both tasks to start. The map is seeded by `post`
        // synchronously so this is purely guarding against a stray race where
        // `await_completion` resolves between `post` and the snapshot below.
        let snap = w.progress_snapshot();
        assert_eq!(snap.active.len(), 2, "two queued tasks should be active");
        assert!(snap.active.iter().any(|p| p.task_id == id1));
        assert!(snap.active.iter().any(|p| p.task_id == id2));

        let _ = tokio::join!(w.await_completion(id1), w.await_completion(id2));
    }

    /// A handler that returns `TaskOutcome::Err` (or panics) surfaces in
    /// `recent_complete` with the `Failed` state and the error message.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn panic_emits_failed_state() {
        let w = make_worker_default(pool().await);
        let id = w.post(Task::Test {
            tag: "panicker",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: Some(Arc::new(|| panic!("intentional test panic"))),
            on_done: None,
        });
        let _ = w.await_completion(id).await;

        // The spawned future has unwound and our terminal guard wrote the
        // `Failed` state; the entry now sits in `recent_complete`.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = w.progress_snapshot();
            if let Some(p) = snap.recent_complete.iter().find(|p| p.task_id == id) {
                match &p.state {
                    ProgressState::Failed { message } => {
                        assert!(
                            message.contains("panic"),
                            "expected panic message, got {message:?}"
                        );
                        return;
                    }
                    other => panic!("expected Failed, got {other:?}"),
                }
            }
            assert!(
                Instant::now() < deadline,
                "timeout waiting for failed terminal"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Terminal entries get GC'd by `progress_snapshot` after the retention
    /// window. Uses the test-only `progress_snapshot_with_retention` so we
    /// can exercise the eviction path with a 100 ms window instead of
    /// sleeping for the full production 10 s.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn progress_snapshot_evicts_terminals_after_retention() {
        let w = make_worker_default(pool().await);
        let id = w.post(Task::Test {
            tag: "evict",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        let _ = w.await_completion(id).await;

        // Right after completion: present in recent_complete with a
        // generous retention window so eviction is opt-in.
        let test_retention = Duration::from_millis(100);
        let snap = w.progress_snapshot_with_retention(Duration::from_secs(60));
        assert!(snap.recent_complete.iter().any(|p| p.task_id == id));

        // Cross the configured window and re-snapshot.
        tokio::time::sleep(test_retention + Duration::from_millis(50)).await;
        let snap2 = w.progress_snapshot_with_retention(test_retention);
        assert!(
            !snap2.recent_complete.iter().any(|p| p.task_id == id),
            "terminal entry should be evicted after retention"
        );
        assert_eq!(w.progress_len(), 0, "progress map should be empty");
    }

    /// `report_progress` is the phase-2 seam used to surface per-EPUB
    /// counts mid-scan. Exercising it here keeps it from being dropped
    /// as dead code and pins down the "ignored after terminal" invariant.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn report_progress_updates_running_count() {
        let w = make_worker_default(pool().await);
        let id = w.post(Task::Test {
            tag: "report",
            latency_ms: 50,
            resource: Some("report".into()),
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        // Pretend we're mid-scan and have processed 3 of 10.
        w.report_progress(id, 3, Some(10));
        let snap = w.progress_snapshot();
        let entry = snap
            .active
            .iter()
            .find(|p| p.task_id == id)
            .expect("running entry");
        assert!(matches!(
            entry.state,
            ProgressState::Running {
                processed: 3,
                total: Some(10)
            }
        ));
        let _ = w.await_completion(id).await;

        // After completion, the entry is terminal and further reports are
        // ignored (the run loop's terminal write is authoritative).
        w.report_progress(id, 99, Some(10));
        let snap2 = w.progress_snapshot();
        let entry2 = snap2
            .recent_complete
            .iter()
            .find(|p| p.task_id == id)
            .expect("terminal entry");
        assert!(matches!(entry2.state, ProgressState::Done { .. }));
    }

    /// Poisoning the `progress` mutex (a thread panics while holding its
    /// lock) must not turn every later worker operation into a panic. Before
    /// the `lock_unpoison` helper a single poisoning event cascaded into a
    /// process-wide crash on the hot path every `Task::Scan` goes through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn poisoned_progress_lock_recovers_instead_of_panicking() {
        let w = make_worker_default(pool().await);

        // Poison the `progress` mutex: take the lock and panic while holding
        // it inside `catch_unwind`, which contains the unwind to this call (no
        // thread is spawned) so the test thread survives and the lock stays
        // poisoned afterward.
        let progress = w.progress.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = progress.lock().unwrap();
            panic!("intentional poisoning panic");
        }));
        assert!(
            w.progress.lock().is_err(),
            "progress mutex should be poisoned after the panic"
        );

        // A full post → await round-trip touches the poisoned `progress`
        // lock in `post` (seed) and `run` (terminal write). It must run to
        // completion rather than panicking.
        let id = w.post(Task::Test {
            tag: "after-poison",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        match w.await_completion(id).await {
            TaskOutcome::Ok => {}
            other => panic!("expected Ok after poison recovery, got {other:?}"),
        }

        // The read path (`progress_snapshot`) also recovers and observes the
        // task's terminal state.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = w.progress_snapshot();
            if snap.recent_complete.iter().any(|p| p.task_id == id) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot never surfaced the post-poison task"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Regression for the poisoned-`progress` drop path: when a task panics
    /// *and* the `progress` mutex is already poisoned, `ProgressTerminalGuard`
    /// must still record the terminal `Failed` entry. Before the drop used
    /// `lock_unpoison` it skipped on poison, leaving the task stuck in
    /// `Running` forever (never evicted, UI shows a stuck task).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn poisoned_progress_lock_still_records_terminal_on_panic() {
        let w = make_worker_default(pool().await);

        // Poison the `progress` mutex (panic while holding the lock, contained
        // by `catch_unwind` so the test thread survives).
        let progress = w.progress.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = progress.lock().unwrap();
            panic!("intentional poisoning panic");
        }));
        assert!(
            w.progress.lock().is_err(),
            "progress mutex should be poisoned after the panic"
        );

        // A task that panics in `run` so its `ProgressTerminalGuard` drops on
        // the unwind — with the lock poisoned, that drop is the only thing
        // that can record the terminal state.
        let id = w.post(Task::Test {
            tag: "panic-after-poison",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: Some(Arc::new(|| panic!("intentional test panic"))),
            on_done: None,
        });
        let _ = w.await_completion(id).await;

        // The entry must surface as terminal `Failed`, never stuck in the
        // `active` (Running) bucket.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = w.progress_snapshot();
            if let Some(p) = snap.recent_complete.iter().find(|p| p.task_id == id) {
                assert!(
                    matches!(p.state, ProgressState::Failed { .. }),
                    "panicked task should be terminal Failed, got {:?}",
                    p.state
                );
                break;
            }
            assert!(
                snap.active.iter().all(|p| p.task_id != id),
                "task stuck in Running after panic with a poisoned progress lock"
            );
            assert!(
                Instant::now() < deadline,
                "terminal entry never surfaced after panic + poison"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Same recovery guarantee for the `completions` mutex, which `post`
    /// and `await_completion` touch on the awaited (non-fire-and-forget)
    /// path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn poisoned_completions_lock_recovers_instead_of_panicking() {
        let w = make_worker_default(pool().await);

        let completions = w.completions.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = completions.lock().unwrap();
            panic!("intentional poisoning panic");
        }));
        assert!(
            w.completions.lock().is_err(),
            "completions mutex should be poisoned after the panic"
        );

        let id = w.post(Task::Test {
            tag: "after-poison",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        match w.await_completion(id).await {
            TaskOutcome::Ok => {}
            other => panic!("expected Ok after poison recovery, got {other:?}"),
        }
    }
}
