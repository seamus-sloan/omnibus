//! Unit tests for the admin-health data layer: index status, FTS parity,
//! storage utilization, and the worker-queue projection.

use omnibus_shared::TaskKind;

use super::*;
use crate::test_support::{seed_minimal_books, CoversTempDir, EnvVarGuard};
use crate::worker::{Task, WorkerConfig};

async fn pool() -> sqlx::SqlitePool {
    crate::pool::init_db("sqlite::memory:").await.unwrap()
}

// ---------- index_status ----------

#[tokio::test]
async fn index_status_reports_zero_counts_and_no_paths_on_an_empty_db() {
    let pool = pool().await;
    let status = index_status(&pool).await.unwrap();
    assert_eq!(status.book_count, 0);
    assert_eq!(status.author_count, 0);
    assert_eq!(status.series_count, 0);
    assert_eq!(status.ghosted_book_count, 0);
    assert_eq!(status.ebook_library_path, None);
    assert_eq!(status.audiobook_library_path, None);
}

#[tokio::test]
async fn index_status_reports_configured_paths_book_counts_and_ghosted_books() {
    let pool = pool().await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('ebook_library_path', '/lib')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO authors (name) VALUES ('Author A'), ('Author B')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO series (name) VALUES ('Series A')")
        .execute(&pool)
        .await
        .unwrap();
    seed_minimal_books(&pool, 3).await;
    // Ghost one book by dropping its `book_files` row — identity preserved,
    // content gone (the F2 shape migration `0026` introduced).
    sqlx::query(
        "DELETE FROM book_files WHERE book_id = (SELECT id FROM books WHERE uuid = 'uuid-1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let status = index_status(&pool).await.unwrap();
    assert_eq!(status.ebook_library_path.as_deref(), Some("/lib"));
    assert_eq!(status.audiobook_library_path, None);
    assert_eq!(status.book_count, 3);
    assert_eq!(status.author_count, 2);
    assert_eq!(status.series_count, 1);
    assert_eq!(status.ghosted_book_count, 1);
}

#[tokio::test]
async fn index_status_propagates_db_error_when_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;
    let err = index_status(&pool).await.unwrap_err();
    assert!(matches!(
        err,
        AdminHealthError::Db(_) | AdminHealthError::Settings(_)
    ));
}

// ---------- fts_health ----------

#[tokio::test]
async fn fts_health_reports_in_sync_on_an_empty_db() {
    let pool = pool().await;
    let health = fts_health(&pool).await.unwrap();
    assert_eq!(health.books_rows, 0);
    assert_eq!(health.fts_rows, 0);
    assert!(health.in_sync);
}

#[tokio::test]
async fn fts_health_reports_out_of_sync_when_books_fts_lags_books() {
    let pool = pool().await;
    // `seed_minimal_books` bypasses the indexer entirely, so it never
    // populates `books_fts` — exactly the drift this section exists to
    // surface.
    seed_minimal_books(&pool, 3).await;
    let health = fts_health(&pool).await.unwrap();
    assert_eq!(health.books_rows, 3);
    assert_eq!(health.fts_rows, 0);
    assert!(!health.in_sync);
}

#[tokio::test]
async fn fts_health_propagates_db_error_when_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;
    let err = fts_health(&pool).await.unwrap_err();
    assert!(matches!(err, AdminHealthError::Db(_)));
}

// ---------- storage_stats ----------

#[tokio::test]
async fn storage_stats_sums_library_bytes_from_book_files() {
    let pool = pool().await;
    // Every seeded book gets one 1-byte `book_files` row.
    seed_minimal_books(&pool, 4).await;
    let _covers = CoversTempDir::new("admin_health_storage_library_bytes");

    let stats = storage_stats(&pool).await.unwrap();
    assert_eq!(stats.library_bytes, 4);
}

#[tokio::test]
async fn storage_stats_sums_covers_thumbs_and_hls_dir_bytes() {
    let pool = pool().await;
    let covers_dir = tempfile::tempdir().unwrap();
    std::fs::write(covers_dir.path().join("book-a.jpg"), vec![0u8; 30]).unwrap();

    let thumbs_dir = tempfile::tempdir().unwrap();
    std::fs::write(thumbs_dir.path().join("1_sm.webp"), vec![0u8; 20]).unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let hls_book_dir = data_dir.path().join("hls").join("1").join("audio64");
    std::fs::create_dir_all(&hls_book_dir).unwrap();
    std::fs::write(hls_book_dir.join("seg0.ts"), vec![0u8; 10]).unwrap();

    let export_epub_dir = tempfile::tempdir().unwrap();
    std::fs::write(export_epub_dir.path().join("1.epub"), vec![0u8; 15]).unwrap();

    // One `EnvVarGuard` chain (not a `CoversTempDir` plus separate
    // `EnvVarGuard`s): both hold the same process-wide `ENV_LOCK`, and it's
    // a plain `std::sync::Mutex` — not reentrant — so acquiring it a second
    // time on this thread while the first guard is still alive deadlocks.
    let _guard = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.path().as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.path().as_os_str()))
        .also_set_os("OMNIBUS_DATA_DIR", Some(data_dir.path().as_os_str()))
        .also_set_os(
            "OMNIBUS_EXPORT_EPUB_DIR",
            Some(export_epub_dir.path().as_os_str()),
        );

    let stats = storage_stats(&pool).await.unwrap();
    assert_eq!(stats.covers_bytes, 30);
    assert_eq!(stats.thumbs_bytes, 20);
    assert_eq!(stats.hls_bytes, 10);
    assert_eq!(stats.export_epub_bytes, 15);
    assert!(stats.thumbs_cap_bytes > 0);
    assert!(stats.hls_cap_bytes > 0);
    assert!(stats.export_epub_cap_bytes > 0);
}

#[tokio::test]
async fn storage_stats_propagates_db_error_when_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;
    let err = storage_stats(&pool).await.unwrap_err();
    assert!(matches!(err, AdminHealthError::Db(_)));
}

// ---------- worker_queue_status ----------

#[tokio::test]
async fn worker_queue_status_is_empty_for_an_idle_worker() {
    let w = crate::worker::Worker::new(pool().await, WorkerConfig::default());
    assert!(worker_queue_status(&w).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_queue_status_reports_a_completed_tasks_kind_and_timing() {
    let w = crate::worker::Worker::new(pool().await, WorkerConfig::default());
    let id = w.post(Task::Test {
        tag: "admin-health-probe",
        latency_ms: 5,
        resource: None,
        route_through_scan_sem: false,
        on_run: None,
        on_done: None,
    });
    w.await_completion(id).await;

    let entries = worker_queue_status(&w);
    assert_eq!(entries.len(), 1);
    // `Task::Test` maps to `TaskKind::Scan` (see `worker::types::Task::kind`).
    assert_eq!(entries[0].kind, TaskKind::Scan);
    assert_eq!(entries[0].queue_depth, 0);
    assert_eq!(entries[0].recent_completions_ms.len(), 1);
}

// ---------- build_report ----------

#[tokio::test]
async fn build_report_composes_every_section_from_one_pool_and_worker() {
    let pool = pool().await;
    let _covers = CoversTempDir::new("admin_health_build_report");
    seed_minimal_books(&pool, 2).await;
    let w = crate::worker::Worker::new(pool.clone(), WorkerConfig::default());

    // `last_errors` is deliberately not asserted here: `crate::error_ring`
    // is one process-wide buffer shared with `error_ring::tests`, which
    // legitimately mutates it concurrently under its own `TEST_LOCK` — a
    // lock this module has no access to. `build_report`'s use of it is a
    // one-line `error_ring::snapshot()` pass-through already covered by
    // `error_ring::tests`.
    let report = build_report(&pool, &w).await.unwrap();
    assert_eq!(report.index.book_count, 2);
    assert!(
        !report.fts.in_sync,
        "seed_minimal_books never populates FTS"
    );
    assert!(report.worker_queue.is_empty(), "no tasks were posted");
}
