//! Unit tests for the forward-progress ledger: baselining, forward accrual,
//! backward moves, and day bucketing.

use super::*;
use crate::init_db;
use crate::test_support::seed_user;

/// 2023-11-14 22:13:20 UTC.
const T0: i64 = 1_700_000_000;
const DAY: i64 = 86_400;

const UUID: &str = "uuid-a";

/// The whole ledger for one book, as `(day, percent_gained)` ascending.
async fn days(pool: &SqlitePool, user: i64) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT day, percent_gained FROM reading_progress_daily
         WHERE user_id = ? ORDER BY day",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn mark(pool: &SqlitePool, user: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT percent FROM reading_progress_marks WHERE user_id = ?")
        .bind(user)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn observe(pool: &SqlitePool, user: i64, percent: i64, at: i64) {
    observe_percent(pool, user, UUID, ProgressFormat::Epub, percent, at)
        .await
        .unwrap();
}

#[tokio::test]
async fn observe_percent_baselines_the_first_observation_without_accruing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // A device syncing a book it is already 60% through has not just read 60%
    // of it — the first percent seen is where the measuring starts.
    observe(&pool, user, 60, T0).await;

    assert_eq!(mark(&pool, user).await, Some(60));
    assert!(days(&pool, user).await.is_empty());
}

#[tokio::test]
async fn observe_percent_accrues_the_forward_gain_into_the_observation_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 10, T0).await;
    observe(&pool, user, 25, T0 + 60).await;

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 15)]);
    assert_eq!(mark(&pool, user).await, Some(25));
}

#[tokio::test]
async fn observe_percent_sums_repeated_gains_within_one_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 0, T0).await;
    observe(&pool, user, 4, T0 + 60).await;
    observe(&pool, user, 9, T0 + 120).await;

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 9)]);
}

#[tokio::test]
async fn observe_percent_accrues_nothing_when_the_position_moves_backward() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 40, T0).await;
    observe(&pool, user, 30, T0 + 60).await;

    // No row at all — negative pages read is not a thing. The mark follows the
    // reader back so the ground re-covered accrues again on the way forward.
    assert!(days(&pool, user).await.is_empty());
    assert_eq!(mark(&pool, user).await, Some(30));

    observe(&pool, user, 45, T0 + 120).await;
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 15)]);
}

#[tokio::test]
async fn observe_percent_accrues_nothing_when_the_position_does_not_move() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 40, T0).await;
    observe(&pool, user, 40, T0 + 60).await;

    assert!(days(&pool, user).await.is_empty());
}

#[tokio::test]
async fn observe_percent_buckets_each_gain_into_its_own_utc_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 0, T0).await;
    observe(&pool, user, 10, T0 + 60).await;
    observe(&pool, user, 30, T0 + DAY).await;

    assert_eq!(
        days(&pool, user).await,
        vec![("2023-11-14".into(), 10), ("2023-11-15".into(), 20)]
    );
}

#[tokio::test]
async fn observe_percent_keeps_the_two_formats_on_separate_marks() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe_percent(&pool, user, UUID, ProgressFormat::Epub, 10, T0)
        .await
        .unwrap();
    // A second format on the same book starts its own baseline rather than
    // differencing against the first — 40% of the audiobook is not 30% of
    // reading.
    observe_percent(&pool, user, UUID, ProgressFormat::Audio, 40, T0)
        .await
        .unwrap();

    assert!(days(&pool, user).await.is_empty());
}

#[tokio::test]
async fn observe_percent_clamps_a_percent_outside_the_stored_range() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // The column CHECK would make a stray value a 500 on the *position* write,
    // which is the write that matters; clamping loses a page of telemetry
    // instead.
    observe(&pool, user, 0, T0).await;
    observe(&pool, user, 140, T0 + 60).await;

    assert_eq!(mark(&pool, user).await, Some(100));
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 100)]);
}

#[tokio::test]
async fn observe_percent_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    pool.close().await;

    let err = observe_percent(&pool, user, UUID, ProgressFormat::Epub, 10, T0)
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)), "got {err:?}");
}

#[tokio::test]
async fn pages_ledger_epoch_returns_the_day_the_migration_recorded() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let epoch = pages_ledger_epoch(&pool).await.unwrap().expect("epoch row");
    // Written as `date('now')` by migration 0083 — assert the shape, since the
    // value is whatever day the test runs on.
    assert_eq!(epoch.len(), 10, "expected YYYY-MM-DD, got {epoch}");
    assert_eq!(epoch.matches('-').count(), 2, "got {epoch}");
}

#[tokio::test]
async fn pages_ledger_epoch_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;

    let err = pages_ledger_epoch(&pool).await.unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)), "got {err:?}");
}
