//! Integration tests for the worker primitive. Stress-tests concurrency,
//! resource serialization, panic / poison recovery, and the
//! progress-snapshot eviction window — all of which are the acceptance
//! gates for the worker submodule split.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::sync::Mutex as TestMutex;
use std::time::{Duration, Instant};

use omnibus_shared::{GhostFilesWarning, MetadataOverrides, ProgressState, TaskKind};
use sqlx::SqlitePool;

use crate::ebook::test_support::copy_fixture_into;
use crate::sync::{sync_books, SyncPlan};
use crate::test_support::{indexed_with_stat, make_test_dir, EnvVarGuard};

use super::types::{Task, TaskOutcome, TaskSuccessDetail, Worker, WorkerConfig};

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
        TaskOutcome::Ok(_) => {}
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
    assert!(matches!(o1, TaskOutcome::Ok(_)));
    assert!(matches!(o2, TaskOutcome::Ok(_)));
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
        TaskOutcome::Ok(_) => {}
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

    // Gate the task inside `on_run` so it cannot finish — and therefore
    // cannot prune its `completions` slot — until `await_completion` has
    // taken the receiver out of the map. Otherwise the 0-latency task can
    // complete and prune first, and `await_completion` documents that an
    // already-pruned id returns `Err("unknown task id")`: a timing race that
    // flakes under CI load.
    let gate = Arc::new(std::sync::Barrier::new(2));
    let gate_task = gate.clone();
    let id = w.post(Task::Test {
        tag: "after-poison",
        latency_ms: 0,
        resource: None,
        route_through_scan_sem: false,
        on_run: Some(Arc::new(move || {
            gate_task.wait();
        })),
        on_done: None,
    });

    // `await_completion` removes the receiver from the map on its first poll,
    // before its first `.await`; releasing the gate only after that yield
    // guarantees the awaiter is holding the receiver and observes the outcome
    // the task then publishes, instead of racing the prune. The barrier wait
    // runs on the blocking pool so it never pins a runtime worker thread.
    let (outcome, ()) = tokio::join!(w.await_completion(id), async {
        tokio::task::yield_now().await;
        tokio::task::spawn_blocking(move || {
            gate.wait();
        })
        .await
        .unwrap();
    });
    match outcome {
        TaskOutcome::Ok(_) => {}
        other => panic!("expected Ok after poison recovery, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_to_kindle_task_fails_when_smtp_unconfigured() {
    // Route through the real dispatch arm: with no SMTP config the handler
    // returns a Failed outcome carrying the "not configured" message.
    let _env =
        crate::test_support::EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let w = make_worker_default(pool().await);
    let id = w.post(Task::SendToKindle {
        book_id: 1,
        book_file_id: None,
        recipient_email: "reader@kindle.com".into(),
    });
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => assert!(msg.contains("not configured"), "got: {msg}"),
        other => panic!("expected Err, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_state_reports_running_then_terminal() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "peek",
        latency_ms: 60,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });

    // Seeded as Running the instant `post` returns, before the task finishes.
    assert!(
        matches!(w.task_state(id), Some(ProgressState::Running { .. })),
        "expected Running immediately after post, got {:?}",
        w.task_state(id)
    );

    match w.await_completion(id).await {
        TaskOutcome::Ok(_) => {}
        other => panic!("expected Ok, got {other:?}"),
    }
    assert!(
        matches!(w.task_state(id), Some(ProgressState::Done { .. })),
        "expected Done after completion, got {:?}",
        w.task_state(id)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_state_is_none_for_unknown_id() {
    let w = make_worker_default(pool().await);
    assert!(w.task_state(9999).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owned_task_state_scopes_reads_to_the_owner() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "owned",
        latency_ms: 60,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    w.set_task_owner(id, 42);

    // The owner sees the state; other users and unowned ids see nothing.
    assert!(w.owned_task_state(id, 42).is_some());
    assert!(
        w.owned_task_state(id, 7).is_none(),
        "a non-owner must not read another user's task state"
    );
    assert!(w.owned_task_state(9999, 42).is_none());

    let _ = w.await_completion(id).await;
}

/// Reproduces the #1163 leak: `rpc_worker_status` is `AuthUser`-scoped (not
/// `AdminUser`), so any authenticated user could previously read another
/// user's owned task off the general worker-status poll — the same
/// guessable-task-id concern [`Worker::owned_task_state`] already guards
/// for the dedicated Kindle-status poll. `owner_scoped_snapshot` must apply
/// the same rule while still surfacing unowned, library-wide tasks (a scan)
/// to every caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_scoped_snapshot_hides_another_users_owned_task_but_keeps_unowned_ones() {
    let w = make_worker_default(pool().await);

    // An owned task (e.g. Send-to-Kindle) that failed with sensitive text.
    let owned_id = w.post(Task::Test {
        tag: "owned",
        latency_ms: 5,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    w.set_task_owner(owned_id, 1);
    let _ = w.await_completion(owned_id).await;
    w.write_terminal_progress(
        owned_id,
        ProgressState::Failed {
            message: "SMTP delivery failed: internal transport detail".to_string(),
        },
    );

    // An unowned, library-wide task (e.g. a scan) any user should still see.
    let shared_id = w.post(Task::Test {
        tag: "shared",
        latency_ms: 5,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    let _ = w.await_completion(shared_id).await;

    // The owner sees their own task's outcome, error text included.
    let owner_view = w.owner_scoped_snapshot(1);
    assert!(owner_view
        .recent_complete
        .iter()
        .any(|p| p.task_id == owned_id));

    // A different, non-admin user must not see the owned task at all — not
    // even to leak that it exists — while the shared task stays visible.
    let other_view = w.owner_scoped_snapshot(2);
    assert!(
        !other_view
            .recent_complete
            .iter()
            .any(|p| p.task_id == owned_id),
        "a non-owner must not see another user's task on the general worker-status poll"
    );
    assert!(other_view
        .recent_complete
        .iter()
        .any(|p| p.task_id == shared_id));
}

// `periodic_scan_tick` tests moved to `worker::periodic_scan::tests`, a
// sibling of `periodic_scan.rs`.

// ---------- #1057: mass-missing warning plumbed onto Task::Scan's Done ----------

/// Seed `count` on-disk stub ebooks under `library_path` through the real
/// `sync_books` write path, each with its actual on-disk `(mtime, size)`
/// stat so a later `Task::Scan` classifies an untouched file as Unchanged.
async fn seed_stub_ebooks(pool: &SqlitePool, library_path: &str, count: usize) {
    for i in 0..count {
        let filename = format!("book-{i}.epub");
        let abs = std::path::Path::new(library_path).join(&filename);
        std::fs::write(&abs, b"not a zip").unwrap();
        let meta = std::fs::metadata(&abs).unwrap();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let book = indexed_with_stat(&filename, Some(&filename), mtime, meta.len() as i64);
        sync_books(
            pool,
            library_path,
            SyncPlan {
                new_books: vec![book],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
}

/// AC2: a scan that ghosts only a few books (below the warn threshold)
/// completes with the ordinary `Done { ghost_warning: None }` — the
/// existing `DoneRow` behavior is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_scan_reports_no_ghost_warning_below_the_warn_threshold() {
    let pool = pool().await;
    let lib = make_test_dir("worker-scan-no-warning");
    let library_path = lib.to_string_lossy().into_owned();
    seed_stub_ebooks(&pool, &library_path, 20).await;
    // 3 of 20 ghosted books is under MASS_MISSING_MIN_ABSOLUTE (10) — always
    // silent, regardless of the 15% fraction.
    for i in 0..3 {
        std::fs::remove_file(lib.join(format!("book-{i}.epub"))).unwrap();
    }

    let w = make_worker_default(pool);
    let id = w.post(Task::Scan { library_path });
    match w.await_completion(id).await {
        TaskOutcome::Ok(detail) => assert_eq!(detail, None),
        other => panic!("expected Ok, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC1/AC5: a scan that ghosts a large-but-sub-abort-threshold number of
/// books completes successfully (the #819 abort guard does not trip) but
/// its `Done` state carries a [`GhostFilesWarning`] naming the ghost count
/// and the pre-scan file-backed total — the wire type the settings page
/// renders a distinct warning row from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_scan_reports_ghost_warning_in_the_warn_band_below_abort() {
    let pool = pool().await;
    let lib = make_test_dir("worker-scan-warning-band");
    let library_path = lib.to_string_lossy().into_owned();
    seed_stub_ebooks(&pool, &library_path, 100).await;
    // 15 of 100 (15%) clears the 10% warn fraction but stays under the 20%
    // abort fraction — the sub-abort middle ground this issue adds.
    for i in 0..15 {
        std::fs::remove_file(lib.join(format!("book-{i}.epub"))).unwrap();
    }

    let w = make_worker_default(pool);
    let id = w.post(Task::Scan { library_path });
    match w.await_completion(id).await {
        TaskOutcome::Ok(detail) => {
            assert_eq!(
                detail,
                Some(TaskSuccessDetail::GhostFiles(GhostFilesWarning {
                    removed: 15,
                    total: 100,
                }))
            );
        }
        other => panic!("expected Ok with a ghost warning, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&lib);
}

// ---------- #1739: bulk EPUB-bake per-book errors on Task::RewriteAllEpubs ----------

/// Insert a `books` row backed by a `book_files` EPUB entry, mirroring
/// `epub_rewrite::tests::seed_epub_row` — the sibling helper that module
/// uses to drive `rewrite_all_epubs_with_overrides` directly. Duplicated
/// rather than shared across crate-internal test modules since it's a
/// handful of inserts with no reuse-worthy behavior of its own.
async fn seed_epub_row_for_bake(
    pool: &SqlitePool,
    lib_dir: &std::path::Path,
    uuid: &str,
    title: &str,
    filename_stem: &str,
) -> i64 {
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(lib_dir.to_string_lossy().to_string())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?)")
            .bind(uuid)
            .bind(lib_id)
            .bind(title)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(filename_stem)
    .execute(pool)
    .await
    .unwrap();
    book_id
}

/// A `Task::RewriteAllEpubs` run that leaves one book unbaked (its
/// `book_files` row points at a source that was never written to disk)
/// completes as `TaskOutcome::Ok`, and the per-book failure — otherwise
/// only logged — rides through as `TaskSuccessDetail::BakeErrors` (#1739).
/// A book with a real fixture on disk bakes successfully alongside it, so
/// the run doesn't abort on the first failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_bake_errors_via_task_outcome() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = pool().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let lib_ok = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib_ok.path());
    seed_epub_row_for_bake(&pool, lib_ok.path(), "uuid-ok", "Book OK", "alpha").await;
    crate::upsert_metadata_overrides(
        &pool,
        "uuid-ok",
        &MetadataOverrides {
            title: Some("Fixed".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let lib_bad = tempfile::tempdir().unwrap();
    seed_epub_row_for_bake(&pool, lib_bad.path(), "uuid-bad", "Book Bad", "missing").await;
    crate::upsert_metadata_overrides(
        &pool,
        "uuid-bad",
        &MetadataOverrides {
            title: Some("Never Applied".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let w = make_worker_default(pool);
    let id = w.post(Task::RewriteAllEpubs);
    match w.await_completion(id).await {
        TaskOutcome::Ok(Some(TaskSuccessDetail::BakeErrors(errors))) => {
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert_eq!(errors[0].book_uuid, "uuid-bad");
            assert!(!errors[0].message.is_empty());
        }
        other => panic!("expected Ok with bake errors, got {other:?}"),
    }
}

/// A `Task::RewriteAllEpubs` run with nothing to bake (no override rows at
/// all) reports the ordinary `TaskOutcome::Ok(None)` — the same shape every
/// other error-free task kind reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_ok_none_when_nothing_fails() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::RewriteAllEpubs);
    assert!(matches!(
        w.await_completion(id).await,
        TaskOutcome::Ok(None)
    ));
}

/// A `Task::RewriteAllEpubs` run against a closed pool can't even resolve
/// the batch — that failure reaches `handle_rewrite_all_epubs`'s `Err` arm
/// and comes back as a sanitized `TaskOutcome::Err`, never the raw
/// `sqlx`/`BooksError` text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_sanitized_err_when_the_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;

    let w = make_worker_default(pool);
    let id = w.post(Task::RewriteAllEpubs);
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => {
            assert!(msg.contains("epub override bake"), "{msg}");
            assert!(!msg.to_lowercase().contains("sqlx"), "{msg}");
        }
        other => panic!("expected Err, got {other:?}"),
    }
}

// ---------- #953: `Worker::metrics` (queue depth + recent completions) ----------

/// `metrics().queue_depth` reflects a task's own [`TaskKind`] while it's
/// still queued/running, and drops back to zero once it completes — mirrors
/// `queued_task_shows_as_running_immediately`'s use of `progress_snapshot`
/// but asserts the aggregate accessor instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_reports_queue_depth_for_in_flight_tasks() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "depth",
        latency_ms: 80,
        resource: Some("depth".into()),
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });

    // Test tasks report as TaskKind::Scan (see `Task::kind`'s doc comment).
    let m = w.metrics();
    assert_eq!(m.queue_depth.get(&TaskKind::Scan), Some(&1));

    let _ = w.await_completion(id).await;
    let m_after = w.metrics();
    assert_eq!(
        m_after
            .queue_depth
            .get(&TaskKind::Scan)
            .copied()
            .unwrap_or(0),
        0,
        "a completed task must not still count against queue depth"
    );
}

/// Two concurrently queued tasks of the same kind both count toward that
/// kind's depth — the accessor aggregates, not just reports presence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_aggregates_queue_depth_across_same_kind_tasks() {
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
    let id1 = mk("m1");
    let id2 = mk("m2");

    let m = w.metrics();
    assert_eq!(m.queue_depth.get(&TaskKind::Scan), Some(&2));

    let _ = tokio::join!(w.await_completion(id1), w.await_completion(id2));
}

/// A completed task's duration lands in `recent_completions` for its
/// `TaskKind`, and `progress_snapshot` keeps reporting the same terminal
/// state unchanged — `metrics()` is purely additive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_records_a_recent_completion_timing_without_disturbing_progress() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "timing",
        latency_ms: 20,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    let _ = w.await_completion(id).await;

    let m = w.metrics();
    let timings = m
        .recent_completions
        .get(&TaskKind::Scan)
        .expect("one completion recorded for TaskKind::Scan");
    assert_eq!(timings.len(), 1);

    // progress_snapshot is unaffected: the terminal entry is still present
    // and still Done, same as before metrics() was ever called.
    let snap = w.progress_snapshot();
    let entry = snap
        .recent_complete
        .iter()
        .find(|p| p.task_id == id)
        .expect("terminal entry still present in progress_snapshot");
    assert!(matches!(entry.state, ProgressState::Done { .. }));
}

/// The recent-completions window per `TaskKind` is bounded: posting more
/// than the cap evicts the oldest entries, keeping only the most recent
/// `RECENT_COMPLETIONS_CAP` durations.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_bounds_the_recent_completions_window_per_kind() {
    let w = make_worker_default(pool().await);
    // One more than the cap (20) so eviction must have kicked in at least
    // once; each task is posted and awaited serially so ordering is stable.
    for _ in 0..21 {
        let id = w.post(Task::Test {
            tag: "bound",
            latency_ms: 0,
            resource: None,
            route_through_scan_sem: false,
            on_run: None,
            on_done: None,
        });
        let _ = w.await_completion(id).await;
    }

    let m = w.metrics();
    let timings = m
        .recent_completions
        .get(&TaskKind::Scan)
        .expect("completions recorded for TaskKind::Scan");
    assert_eq!(
        timings.len(),
        20,
        "recent-completions window must stay capped at 20 entries"
    );
}

/// Different task kinds keep independent queue-depth and recent-completion
/// buckets — a burst of one kind must not appear under another's key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_keeps_separate_buckets_per_task_kind() {
    let w = make_worker_default(pool().await);
    let scan_id = w.post(Task::Test {
        tag: "scan-kind",
        latency_ms: 0,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    let _ = w.await_completion(scan_id).await;

    let thumb_id = w.post(Task::GenerateThumbs {
        book_id: 1,
        last_modified_epoch: 0,
    });
    let _ = w.await_completion(thumb_id).await;

    let m = w.metrics();
    assert!(
        m.recent_completions.contains_key(&TaskKind::Scan),
        "Test task should record under TaskKind::Scan"
    );
    assert!(
        m.recent_completions.contains_key(&TaskKind::GenerateThumbs),
        "GenerateThumbs task should record under its own TaskKind"
    );
}
