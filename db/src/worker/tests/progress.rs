//! The progress snapshot: queued and concurrent tasks show as running, a
//! panic emits a failed state, terminals evict after the retention window,
//! `report_progress_update` / `report_detail` shape the running entry, and
//! `task_state` reads are scoped to the owning user.

use std::sync::Arc;
use std::time::{Duration, Instant};

use omnibus_shared::{ProgressState, ScanTallies, TaskDetail, TaskKind};

use super::super::types::{Task, TaskOutcome};
use super::{make_worker_default, pool};

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

/// `report_progress_update` is the mid-task seam used to surface per-EPUB
/// counts mid-scan. Exercising it here pins down the "ignored after
/// terminal" invariant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_progress_update_updates_running_count() {
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
    w.report_progress_update(id, 3, Some(10), TaskDetail::default());
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
    w.report_progress_update(id, 99, Some(10), TaskDetail::default());
    let snap2 = w.progress_snapshot();
    let entry2 = snap2
        .recent_complete
        .iter()
        .find(|p| p.task_id == id)
        .expect("terminal entry");
    assert!(matches!(entry2.state, ProgressState::Done { .. }));
}

/// `report_progress_update` stores the verbose detail alongside the
/// counted state, and the terminal write keeps only the tallies (phase
/// and current-item would read as stale on a done row).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_progress_update_stores_detail_and_terminal_write_prunes_to_tallies() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "detail",
        latency_ms: 50,
        resource: Some("detail".into()),
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    let tallies = ScanTallies {
        found: 10,
        new: 3,
        changed: 1,
        removed: 2,
        moved: 0,
        unchanged: 4,
    };
    w.report_progress_update(
        id,
        3,
        Some(10),
        TaskDetail {
            phase: Some("Reading file metadata".into()),
            current_item: Some("books/Author/Title.epub".into()),
            tallies: Some(tallies),
        },
    );
    let snap = w.progress_snapshot();
    let entry = snap
        .active
        .iter()
        .find(|p| p.task_id == id)
        .expect("running entry");
    let detail = entry.detail.as_ref().expect("detail stored");
    assert_eq!(detail.phase.as_deref(), Some("Reading file metadata"));
    assert_eq!(
        detail.current_item.as_deref(),
        Some("books/Author/Title.epub")
    );
    assert_eq!(detail.tallies, Some(tallies));

    let _ = w.await_completion(id).await;
    let snap = w.progress_snapshot();
    let entry = snap
        .recent_complete
        .iter()
        .find(|p| p.task_id == id)
        .expect("terminal entry");
    let detail = entry.detail.as_ref().expect("tallies survive the terminal");
    assert_eq!(detail.phase, None, "phase must be cleared on terminal");
    assert_eq!(
        detail.current_item, None,
        "current_item must be cleared on terminal"
    );
    assert_eq!(detail.tallies, Some(tallies));
}

/// A detail with no tallies is dropped entirely at the terminal write, so
/// the done row carries no empty `detail` object on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_write_drops_detail_without_tallies() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "detail-no-tallies",
        latency_ms: 50,
        resource: Some("detail-no-tallies".into()),
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    w.report_progress_update(
        id,
        1,
        None,
        TaskDetail {
            phase: Some("Walking the library".into()),
            current_item: None,
            tallies: None,
        },
    );
    let _ = w.await_completion(id).await;
    let snap = w.progress_snapshot();
    let entry = snap
        .recent_complete
        .iter()
        .find(|p| p.task_id == id)
        .expect("terminal entry");
    assert_eq!(entry.detail, None, "tally-less detail must not survive");
}

/// `report_detail` replaces only the detail, leaving the counted state
/// alone — the shape single-item tasks (one thumbnail, one author photo)
/// use to name their subject without a processed/total surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_detail_sets_current_item_without_touching_the_counted_state() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::Test {
        tag: "detail-only",
        latency_ms: 50,
        resource: Some("detail-only".into()),
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    w.report_progress_update(id, 3, Some(10), TaskDetail::default());
    w.report_detail(
        id,
        TaskDetail {
            current_item: Some("The Fellowship of the Ring".into()),
            ..TaskDetail::default()
        },
    );
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
    assert_eq!(
        entry
            .detail
            .as_ref()
            .and_then(|d| d.current_item.as_deref()),
        Some("The Fellowship of the Ring")
    );
    let _ = w.await_completion(id).await;
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
