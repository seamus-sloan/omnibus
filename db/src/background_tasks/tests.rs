//! Unit tests for `background_tasks`: insert-on-start, update-on-completion
//! (success and failure), ordering, and the persistence-survives-a-fresh-pool
//! proof for AC2.

use super::*;
use crate::init_db;

#[tokio::test]
async fn start_task_inserts_a_running_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let id = start_task(&pool, "scan", 1_000).await.unwrap();

    let rows = recent_tasks(&pool, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].task_kind, "scan");
    assert_eq!(rows[0].status, BackgroundTaskStatus::Running);
    assert_eq!(rows[0].started_at, 1_000);
    assert_eq!(rows[0].finished_at, None);
    assert_eq!(rows[0].error, None);
}

#[tokio::test]
async fn finish_task_marks_success_with_no_error() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let id = start_task(&pool, "scan", 1_000).await.unwrap();

    finish_task(&pool, id, BackgroundTaskStatus::Success, 1_050, None)
        .await
        .unwrap();

    let rows = recent_tasks(&pool, 10).await.unwrap();
    assert_eq!(rows[0].status, BackgroundTaskStatus::Success);
    assert_eq!(rows[0].finished_at, Some(1_050));
    assert_eq!(rows[0].error, None);
}

#[tokio::test]
async fn finish_task_marks_failed_with_the_error_message() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let id = start_task(&pool, "kepub_convert", 2_000).await.unwrap();

    finish_task(
        &pool,
        id,
        BackgroundTaskStatus::Failed,
        2_010,
        Some("kepub conversion failed; see server logs for details"),
    )
    .await
    .unwrap();

    let rows = recent_tasks(&pool, 10).await.unwrap();
    assert_eq!(rows[0].status, BackgroundTaskStatus::Failed);
    assert_eq!(rows[0].finished_at, Some(2_010));
    assert_eq!(
        rows[0].error.as_deref(),
        Some("kepub conversion failed; see server logs for details")
    );
}

#[tokio::test]
async fn finish_task_is_a_no_op_when_id_is_unknown() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // No row was ever inserted for id 999 — this must not error, since the
    // history row is best-effort observability, not load-bearing state.
    finish_task(&pool, 999, BackgroundTaskStatus::Success, 10, None)
        .await
        .unwrap();
    assert!(recent_tasks(&pool, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn recent_tasks_returns_newest_first_and_respects_the_limit() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    for i in 0..5 {
        start_task(&pool, "scan", 1_000 + i).await.unwrap();
    }

    let rows = recent_tasks(&pool, 3).await.unwrap();
    assert_eq!(rows.len(), 3);
    // Newest (highest id / most recently inserted) first.
    assert!(rows[0].id > rows[1].id && rows[1].id > rows[2].id);
}

#[tokio::test]
async fn background_task_history_survives_a_fresh_pool_reopen_against_the_same_file() {
    // AC2: task history survives a server restart. A fresh `SqlitePool`
    // against the same on-disk file stands in for a process restart — the
    // migrator re-runs (idempotently) and the row must still be there.
    // Mirrors `pool::tests::init_db_returns_migrate_error_when_applied_checksum_is_tampered`'s
    // on-disk-restart setup.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let id = {
        let pool = init_db(&url).await.unwrap();
        let id = start_task(&pool, "scan", 5_000).await.unwrap();
        finish_task(&pool, id, BackgroundTaskStatus::Success, 5_010, None)
            .await
            .unwrap();
        drop(pool);
        id
    };

    // Fresh pool, same file: simulates the server process restarting.
    let reopened = init_db(&url).await.unwrap();
    let rows = recent_tasks(&reopened, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].status, BackgroundTaskStatus::Success);
    assert_eq!(rows[0].finished_at, Some(5_010));
}

#[tokio::test]
async fn start_task_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = start_task(&pool, "scan", 0).await.unwrap_err();
    assert!(matches!(err, BackgroundTaskError::Sqlx(_)));
}

#[tokio::test]
async fn finish_task_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let id = start_task(&pool, "scan", 0).await.unwrap();
    pool.close().await;
    let err = finish_task(&pool, id, BackgroundTaskStatus::Success, 1, None)
        .await
        .unwrap_err();
    assert!(matches!(err, BackgroundTaskError::Sqlx(_)));
}

#[tokio::test]
async fn recent_tasks_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = recent_tasks(&pool, 10).await.unwrap_err();
    assert!(matches!(err, BackgroundTaskError::Sqlx(_)));
}
