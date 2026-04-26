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

#[non_exhaustive]
pub enum Task {
    Scan {
        library_path: String,
    },
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
            #[cfg(test)]
            Task::Test { resource, .. } => resource.clone(),
        }
    }

    fn uses_scan_sem(&self) -> bool {
        match self {
            Task::Scan { .. } => true,
            #[cfg(test)]
            Task::Test {
                route_through_scan_sem,
                ..
            } => *route_through_scan_sem,
        }
    }
}

pub type TaskId = u64;

#[derive(Clone, Debug)]
pub enum TaskOutcome {
    Ok,
    Err(String),
}

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub scan_concurrency: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            scan_concurrency: 1,
        }
    }
}

pub struct Worker {
    pool: SqlitePool,
    scan_sem: Arc<Semaphore>,
    resource_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    completions: Arc<StdMutex<HashMap<TaskId, watch::Sender<Option<TaskOutcome>>>>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl Worker {
    pub fn new(pool: SqlitePool, config: WorkerConfig) -> Arc<Self> {
        Arc::new(Self {
            pool,
            scan_sem: Arc::new(Semaphore::new(config.scan_concurrency.max(1))),
            resource_locks: Arc::new(Mutex::new(HashMap::new())),
            completions: Arc::new(StdMutex::new(HashMap::new())),
            next_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    pub fn post(self: &Arc<Self>, task: Task) -> TaskId {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, _rx) = watch::channel(None);
        self.completions.lock().unwrap().insert(id, tx.clone());

        let this = self.clone();
        tokio::spawn(async move {
            let outcome = this.run(task).await;
            let _ = tx.send(Some(outcome));
        });

        id
    }

    pub async fn await_completion(&self, id: TaskId) -> TaskOutcome {
        let mut rx = {
            let map = self.completions.lock().unwrap();
            match map.get(&id) {
                Some(tx) => tx.subscribe(),
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
        let _scan_permit = if task.uses_scan_sem() {
            match self.scan_sem.clone().acquire_owned().await {
                Ok(p) => Some(p),
                Err(_) => return TaskOutcome::Err("scan semaphore closed".into()),
            }
        } else {
            None
        };

        let _resource_guard = if let Some(key) = task.resource_key() {
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

        self.execute(task).await
    }

    async fn execute(&self, task: Task) -> TaskOutcome {
        match task {
            Task::Scan { library_path } => {
                match crate::indexer::reindex(&self.pool, library_path).await {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
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
}
