//! Posting and awaiting tasks: same-resource serialization, cross-resource
//! parallelism, the concurrency cap, `await_completion` after a panic or
//! after the slot was pruned or evicted, and the maps draining for
//! fire-and-forget tasks.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::sync::Mutex as TestMutex;
use std::time::{Duration, Instant};

use super::super::types::{Task, TaskOutcome, Worker, WorkerConfig};
use super::{make_worker_default, poll_maps_empty, pool};

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
            convert_concurrency: 1,
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
        // Pinned verbatim: a never-posted id must stay distinguishable from
        // a task that ran and had its completion slot reclaimed.
        TaskOutcome::Err(msg) => assert_eq!(msg, "unknown task id"),
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

/// A task that finishes before its caller reaches `await_completion` must
/// still report its real outcome. `CompletionsPruneGuard` reclaims the slot
/// the moment the spawned future ends, so the lookup has to fall back to the
/// retained terminal progress entry instead of reporting an unknown id.
/// Draining the maps first forces the losing interleaving rather than hoping
/// for it — the natural race only loses reliably on fast Linux runners.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn await_completion_returns_the_outcome_after_the_slot_was_pruned() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "pruned-before-await",
        latency_ms: 0,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    assert!(poll_maps_empty(&w).await, "task never finished");
    match w.await_completion(id).await {
        TaskOutcome::Ok(None) => {}
        other => panic!("expected Ok(None) from the retained terminal, got {other:?}"),
    }
}

/// Failure sibling of
/// [`await_completion_returns_the_outcome_after_the_slot_was_pruned`]: the
/// recovered outcome carries the real terminal message, so a late awaiter
/// can still tell why the task failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn await_completion_returns_the_failure_after_the_slot_was_pruned() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "pruned-panicker",
        latency_ms: 0,
        resource: None,
        route_through_scan_sem: false,
        on_run: Some(Arc::new(|| panic!("intentional test panic"))),
        on_done: None,
    });
    assert!(poll_maps_empty(&w).await, "task never finished");
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => assert_eq!(msg, "task panicked"),
        other => panic!("expected the recorded failure, got {other:?}"),
    }
}

/// The recovery window is exactly the progress map's retention, not
/// forever: once the terminal entry is evicted the id reads as unknown
/// again, which is what keeps the fallback from turning into a second
/// unbounded map.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn await_completion_reports_unknown_id_once_the_terminal_is_evicted() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "evicted-terminal",
        latency_ms: 0,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    assert!(poll_maps_empty(&w).await, "task never finished");
    // Zero retention evicts the terminal on the very next snapshot read.
    w.progress_snapshot_with_retention(Duration::ZERO);
    assert_eq!(w.progress_len(), 0);

    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => assert_eq!(msg, "unknown task id"),
        other => panic!("expected Err after eviction, got {other:?}"),
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
