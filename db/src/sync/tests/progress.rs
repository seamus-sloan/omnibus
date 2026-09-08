//! The progress-callback contract of `sync_books_with_progress` and
//! `sync_audiobooks_with_progress`: an initial `(0, total)` before any
//! write, monotonic ticks up to a constant `total`, and Removed/Backfill
//! counts excluded from it.

use super::super::*;
use crate::pool::init_db;
use crate::test_support::{indexed, indexed_audiobook, indexed_with_stat, CoversTempDir};

// `sync_books_with_progress` and `sync_audiobooks_with_progress` feed
// the worker's `report_progress_update` so the UI indicator can render a
// determinate `processed / total` bar. The contract: emit `(0, total)`
// before any per-book write, then tick `processed` monotonically up to
// `total` (one tick per New + Changed book), with `total` constant
// across the run. Removed/Backfill counts are deliberately excluded
// from `total` — they're batched and invisible to the user-facing
// "Scanning books… N / M" step.
#[tokio::test]
async fn sync_books_with_progress_emits_initial_zero_and_monotonic_ticks() {
    let _covers = CoversTempDir::new("progress_books");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan = SyncPlan {
        new_books: vec![
            indexed_with_stat("a.epub", Some("A"), 1, 1),
            indexed_with_stat("b.epub", Some("B"), 2, 2),
            indexed_with_stat("c.epub", Some("C"), 3, 3),
        ],
        ..Default::default()
    };
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_books_with_progress(&pool, "/lib", plan, move |p, t, _| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();

    let ticks = ticks.lock().unwrap().clone();
    // First tick is (0, total) so the UI can flip from indeterminate
    // spinner to a determinate bar before the first per-book write.
    assert_eq!(ticks.first(), Some(&(0u32, 3u32)));
    // Last tick is (total, total) — we processed every book.
    assert_eq!(ticks.last(), Some(&(3u32, 3u32)));
    // Monotonic non-decreasing processed counter; constant total.
    for pair in ticks.windows(2) {
        assert!(
            pair[0].0 <= pair[1].0,
            "processed must not regress: {ticks:?}"
        );
        assert_eq!(pair[0].1, pair[1].1, "total must stay constant: {ticks:?}");
    }
}

#[tokio::test]
async fn sync_books_with_progress_reports_zero_total_for_no_op_plan() {
    let _covers = CoversTempDir::new("progress_books_noop");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_books_with_progress(&pool, "/lib", SyncPlan::default(), move |p, t, _| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();
    // Even when there's nothing to do we still emit the initial (0, 0)
    // tick so the UI's "switch to determinate bar" code path runs
    // consistently.
    let ticks = ticks.lock().unwrap().clone();
    assert_eq!(ticks, vec![(0u32, 0u32)]);
}

#[tokio::test]
async fn sync_books_with_progress_excludes_removed_and_backfill_from_total() {
    // Total counts the buckets that loop per-book (Changed + New). The
    // Removed and Backfill phases are batched SQL — reporting them as
    // per-book ticks would either inflate `total` for work the user
    // can't see, or under-count `processed` mid-run.
    let _covers = CoversTempDir::new("progress_books_buckets");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("gone.epub", Some("Gone"), &[], &[], None, None),
            indexed("survivor.epub", Some("Survivor"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let survivor_uuid = crate::test_support::uuid_by_scan_key(&pool, "survivor.epub").await;
    let gone_uuid = crate::test_support::uuid_by_scan_key(&pool, "gone.epub").await;

    let plan = SyncPlan {
        new_books: vec![indexed_with_stat("new.epub", Some("New"), 10, 10)],
        removed_uuids: vec![gone_uuid],
        backfill: vec![(survivor_uuid, 42, 42)],
        ..Default::default()
    };
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_books_with_progress(&pool, "/lib", plan, move |p, t, _| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();

    let ticks = ticks.lock().unwrap().clone();
    // 1 New, 0 Changed → total = 1; Removed + Backfill don't bump it.
    let totals: std::collections::BTreeSet<u32> = ticks.iter().map(|(_, t)| *t).collect();
    assert_eq!(totals.into_iter().collect::<Vec<_>>(), vec![1u32]);
    assert_eq!(ticks.last(), Some(&(1u32, 1u32)));
}

#[tokio::test]
async fn sync_audiobooks_with_progress_emits_initial_zero_and_monotonic_ticks() {
    let _covers = CoversTempDir::new("progress_audiobooks");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan = AudiobookSyncPlan {
        new_books: vec![
            indexed_audiobook("Author/A.m4b", "A", Some("Author")),
            indexed_audiobook("Author/B.m4b", "B", Some("Author")),
        ],
        ..Default::default()
    };
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_audiobooks_with_progress(&pool, "/lib", plan, move |p, t, _| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();

    let ticks = ticks.lock().unwrap().clone();
    assert_eq!(ticks.first(), Some(&(0u32, 2u32)));
    assert_eq!(ticks.last(), Some(&(2u32, 2u32)));
    for pair in ticks.windows(2) {
        assert!(
            pair[0].0 <= pair[1].0,
            "processed must not regress: {ticks:?}"
        );
        assert_eq!(pair[0].1, pair[1].1, "total must stay constant: {ticks:?}");
    }
}
