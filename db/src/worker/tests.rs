//! Integration tests for the worker primitive. Stress-tests concurrency,
//! resource serialization, panic / poison recovery, and the
//! progress-snapshot eviction window — all of which are the acceptance
//! gates for the worker submodule split.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::sync::Mutex as TestMutex;
use std::time::{Duration, Instant};

use omnibus_shared::{ProgressState, TaskKind};
use sqlx::SqlitePool;

use super::types::{Task, TaskOutcome, Worker, WorkerConfig};

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
    let intervals: Arc<TestMutex<Vec<(Instant, Instant)>>> = Arc::new(TestMutex::new(Vec::new()));

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
    let intervals: Arc<TestMutex<Vec<(Instant, Instant)>>> = Arc::new(TestMutex::new(Vec::new()));

    let mk =
        |w: &Arc<Worker>, key: &'static str, intervals: Arc<TestMutex<Vec<(Instant, Instant)>>>| {
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
            hls_concurrency: 1,
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
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if w.completions_len() == 0 && w.resource_locks_len().await == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Like [`poll_maps_empty`] but waits only on `resource_locks`.
async fn poll_resource_locks_empty(w: &Arc<Worker>) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if w.resource_locks_len().await == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
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
