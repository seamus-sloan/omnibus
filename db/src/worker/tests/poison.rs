//! Lock-poison recovery: a poisoned progress or completions mutex is
//! recovered rather than propagated, and a terminal is still recorded
//! after a panic under the poisoned lock.

use std::sync::Arc;
use std::time::{Duration, Instant};

use omnibus_shared::ProgressState;

use super::super::types::{Task, TaskOutcome};
use super::{make_worker_default, pool};

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
