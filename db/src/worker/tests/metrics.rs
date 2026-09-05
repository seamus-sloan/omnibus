//! What the worker records about its work: `Worker::metrics` (queue depth
//! per kind, the bounded recent-completions window) and the
//! `background_tasks` row every posted task persists through running,
//! success and failure.

use std::time::{Duration, Instant};

use omnibus_shared::{ProgressState, TaskKind};

use super::super::types::{Task, TaskOutcome};
use super::{make_worker_default, pool};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_persists_a_background_tasks_row_on_success() {
    let pool = pool().await;
    let w = make_worker_default(pool.clone());
    let id = w.post(Task::Test {
        tag: "persist-ok",
        latency_ms: 0,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    match w.await_completion(id).await {
        TaskOutcome::Ok(_) => {}
        other => panic!("expected Ok, got {other:?}"),
    }

    let rows = crate::background_tasks::recent_tasks(&pool, 10)
        .await
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.task_kind == "test")
        .expect("expected a persisted row for the test task");
    assert_eq!(
        row.status,
        omnibus_shared::BackgroundTaskStatus::Success,
        "successful task must persist as Success"
    );
    assert!(row.finished_at.is_some(), "finished_at must be recorded");
    assert!(
        row.error.is_none(),
        "a successful run must not carry an error"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_persists_a_failed_background_tasks_row_with_the_error_message() {
    let pool = pool().await;
    let w = make_worker_default(pool.clone());
    // No book with this id exists in the fresh in-memory DB, so the handler
    // takes its real (non-panic) failure path.
    let id = w.post(Task::GenerateThumbs {
        book_id: 999_999,
        last_modified_epoch: 0,
    });
    let outcome = w.await_completion(id).await;
    let TaskOutcome::Err(expected_msg) = outcome else {
        panic!("expected Err, got {outcome:?}");
    };

    let rows = crate::background_tasks::recent_tasks(&pool, 10)
        .await
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.task_kind == "generate_thumbs")
        .expect("expected a persisted row for the thumbnail task");
    assert_eq!(row.status, omnibus_shared::BackgroundTaskStatus::Failed);
    assert!(row.finished_at.is_some());
    assert_eq!(row.error.as_deref(), Some(expected_msg.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_persists_a_running_row_immediately_after_posting() {
    // AC1's "insert on start" half, observed before the task has had a
    // chance to reach a terminal state: latency keeps the task in flight
    // long enough to read the row while it's still `running`.
    let pool = pool().await;
    let w = make_worker_default(pool.clone());
    let id = w.post(Task::Test {
        tag: "persist-running",
        latency_ms: 200,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let rows = crate::background_tasks::recent_tasks(&pool, 10)
            .await
            .unwrap();
        if rows.iter().any(|r| r.task_kind == "test") {
            let row = rows.iter().find(|r| r.task_kind == "test").unwrap();
            assert_eq!(row.status, omnibus_shared::BackgroundTaskStatus::Running);
            assert!(row.finished_at.is_none());
            break;
        }
        if Instant::now() >= deadline {
            panic!("background_tasks row never appeared for the in-flight task");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Drain the task so it doesn't outlive the test.
    let _ = w.await_completion(id).await;
}
