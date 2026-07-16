//! Tests for [`super::periodic_scan_tick`], extracted from `worker::tests`
//! (#1078) once the growing `periodic_scan_tick` block pushed that file past
//! the 800-line file-shape cap.

use std::sync::Arc;
use std::time::Duration;

use omnibus_shared::Settings;
use sqlx::SqlitePool;

use super::super::types::{Worker, WorkerConfig};
use super::{periodic_scan_tick, PERIODIC_SCAN_RECHECK};

async fn pool() -> SqlitePool {
    crate::init_db("sqlite::memory:").await.unwrap()
}

fn make_worker_default(pool: SqlitePool) -> Arc<Worker> {
    Worker::new(pool, WorkerConfig::default())
}

/// Find a posted task's progress entry by its `resource_key`, checking both
/// the active and just-completed buckets so the assertion doesn't race a
/// fast-failing scan of a nonexistent test path.
fn find_by_resource(w: &Arc<Worker>, resource_key: &str) -> bool {
    let snap = w.progress_snapshot();
    snap.active
        .iter()
        .chain(snap.recent_complete.iter())
        .any(|p| p.resource_key.as_deref() == Some(resource_key))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_scan_tick_skips_and_recommends_a_recheck_wait_when_interval_unset() {
    let p = pool().await;
    crate::settings::set_settings(
        &p,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let w = make_worker_default(p.clone());

    let wait = periodic_scan_tick(&p, &w).await;

    assert_eq!(wait, PERIODIC_SCAN_RECHECK);
    assert!(
        w.progress_snapshot().active.is_empty(),
        "no scan should be posted while scan_interval_hours is unset"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_scan_tick_posts_scan_and_scan_audiobooks_when_due() {
    let p = pool().await;
    crate::settings::set_settings(
        &p,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: Some(6),
        },
    )
    .await
    .unwrap();
    let w = make_worker_default(p.clone());

    let wait = periodic_scan_tick(&p, &w).await;

    assert_eq!(wait, Duration::from_secs(6 * 3600));
    assert!(
        find_by_resource(&w, "/ebooks"),
        "a due tick must post Task::Scan for the configured ebook path"
    );
    assert!(
        find_by_resource(&w, "audiobooks:/audio"),
        "a due tick must post Task::ScanAudiobooks for the configured audiobook path"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_scan_tick_posts_only_the_configured_library_path() {
    let p = pool().await;
    crate::settings::set_settings(
        &p,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: None,
            scan_interval_hours: Some(2),
        },
    )
    .await
    .unwrap();
    let w = make_worker_default(p.clone());

    let _ = periodic_scan_tick(&p, &w).await;

    assert!(find_by_resource(&w, "/ebooks"));
    assert_eq!(
        w.progress_snapshot().active.len(),
        1,
        "no audiobook path configured, so only one scan should be posted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_scan_tick_recommends_a_recheck_wait_when_settings_read_fails() {
    let p = pool().await;
    let w = make_worker_default(p.clone());
    p.close().await;

    let wait = periodic_scan_tick(&p, &w).await;

    assert_eq!(wait, PERIODIC_SCAN_RECHECK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_scan_tick_treats_a_sub_minimum_interval_row_as_disabled_without_a_zero_sleep() {
    // A corrupted/hand-edited "0" row parses as Some(0) — get_settings does
    // not re-validate — so the tick itself must refuse to busy-spin.
    let p = pool().await;
    crate::settings::set_settings(
        &p,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('scan_interval_hours', '0')")
        .execute(&p)
        .await
        .unwrap();
    let w = make_worker_default(p.clone());

    let wait = periodic_scan_tick(&p, &w).await;

    assert_eq!(
        wait, PERIODIC_SCAN_RECHECK,
        "a sub-minimum interval must recheck, never return a zero/sub-minimum sleep"
    );
    assert!(wait > Duration::ZERO);
    assert!(
        w.progress_snapshot().active.is_empty(),
        "no scan should be posted for a sub-minimum interval"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_scan_tick_recommends_a_recheck_wait_when_enabled_but_no_paths_configured() {
    // Interval is valid but no library path is set yet: recheck soon so a
    // path added later isn't stuck behind a full-interval sleep.
    let p = pool().await;
    crate::settings::set_settings(
        &p,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
            scan_interval_hours: Some(6),
        },
    )
    .await
    .unwrap();
    let w = make_worker_default(p.clone());

    let wait = periodic_scan_tick(&p, &w).await;

    assert_eq!(wait, PERIODIC_SCAN_RECHECK);
    assert!(
        w.progress_snapshot().active.is_empty(),
        "no scan should be posted when no library path is configured"
    );
}
