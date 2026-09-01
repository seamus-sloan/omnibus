//! Unit tests for the forward-progress ledger: baselining, forward accrual,
//! the sitting boundary that makes a there-and-back move free, out-of-order
//! observations, and day bucketing.

use super::*;
use crate::init_db;
use crate::test_support::seed_user;

/// 2023-11-14 22:13:20 UTC.
const T0: i64 = 1_700_000_000;
const DAY: i64 = 86_400;

/// One second past the sitting boundary — the smallest gap that opens a new
/// sitting, so a test asking for one can't be read as asking for idleness in
/// general.
const PAST_GAP: i64 = IDLE_GAP_SECS + 1;

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

/// The sitting's high-water percent.
async fn mark(pool: &SqlitePool, user: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT sitting_max_percent FROM reading_progress_marks WHERE user_id = ?")
        .bind(user)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// The sitting clock the gap test reads.
async fn clock(pool: &SqlitePool, user: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT sitting_observed_at FROM reading_progress_marks WHERE user_id = ?")
        .bind(user)
        .fetch_optional(pool)
        .await
        .unwrap()
        .flatten()
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
async fn observe_percent_charges_a_there_and_back_move_only_for_the_new_ground() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // The bug this scoping exists for: at 40%, flip back to 30% to find a
    // quote, return, read on to 45%. Differencing consecutive writes charged
    // the 30→40 stretch a second time and reported 15.
    observe(&pool, user, 40, T0).await;
    observe(&pool, user, 30, T0 + 60).await;
    observe(&pool, user, 45, T0 + 120).await;

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 5)]);
    assert_eq!(mark(&pool, user).await, Some(45));
}

#[tokio::test]
async fn observe_percent_keeps_forward_ground_the_reader_later_backtracks_out_of() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // 40→60 was read, so it stays credited even though the reader then went
    // back and ended the sitting behind it. This is where the high-water mark
    // and an endpoint difference part company: the latter would report 5.
    observe(&pool, user, 40, T0).await;
    observe(&pool, user, 60, T0 + 60).await;
    observe(&pool, user, 30, T0 + 120).await;
    observe(&pool, user, 45, T0 + 180).await;

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 20)]);
    assert_eq!(mark(&pool, user).await, Some(60));
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
async fn observe_percent_opens_a_new_sitting_after_an_idle_gap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 10, T0).await;
    observe(&pool, user, 30, T0 + 60).await;

    // Back at 20 after putting the book down: a new sitting, so it baselines
    // there rather than reading as a backward move inside the old one.
    observe(&pool, user, 20, T0 + 60 + PAST_GAP).await;
    assert_eq!(mark(&pool, user).await, Some(20));
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 20)]);

    observe(&pool, user, 35, T0 + 120 + PAST_GAP).await;
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 35)]);
}

#[tokio::test]
async fn observe_percent_credits_a_reread_in_full_from_its_own_baseline() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // Finished at 100, restarted the next day. The re-read baselines at 5 and
    // accrues from there — a lifetime high-water mark would have swallowed the
    // whole thing, which is why the sitting is the scope and the book is not.
    observe(&pool, user, 100, T0).await;
    observe(&pool, user, 5, T0 + DAY).await;
    observe(&pool, user, 30, T0 + DAY + 60).await;

    assert_eq!(days(&pool, user).await, vec![("2023-11-15".into(), 25)]);
    assert_eq!(mark(&pool, user).await, Some(30));
}

#[tokio::test]
async fn observe_percent_keeps_one_sitting_across_a_gap_of_exactly_the_threshold() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // The boundary is inclusive — `IDLE_GAP_SECS` of quiet is still the same
    // sitting, and only the second past it is not.
    observe(&pool, user, 10, T0).await;
    observe(&pool, user, 20, T0 + IDLE_GAP_SECS).await;
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 10)]);

    observe(&pool, user, 30, T0 + IDLE_GAP_SECS + PAST_GAP).await;
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 10)]);
    assert_eq!(mark(&pool, user).await, Some(30));
}

#[tokio::test]
async fn observe_percent_buckets_each_gain_into_its_own_utc_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 0, T0).await;
    observe(&pool, user, 10, T0 + 60).await;
    // A day later is a new sitting, so it takes a second observation before
    // the next day's reading has a baseline to accrue against.
    observe(&pool, user, 10, T0 + DAY).await;
    observe(&pool, user, 30, T0 + DAY + 60).await;

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
async fn observe_percent_treats_an_out_of_order_observation_as_the_same_sitting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 10, T0).await;
    observe(&pool, user, 30, T0 + 100).await;
    // The off-request-path percent derivation completing late, carrying its
    // own position's older event time. A negative gap is not idleness, so it
    // extends the sitting instead of baselining a new one.
    observe(&pool, user, 35, T0 + 50).await;

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 25)]);
    assert_eq!(mark(&pool, user).await, Some(35));
}

#[tokio::test]
async fn observe_percent_does_not_rewind_the_sitting_clock_when_an_observation_arrives_late() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    observe(&pool, user, 10, T0).await;
    observe(&pool, user, 20, T0 + 800).await;
    observe(&pool, user, 25, T0 + 10).await;

    // Clamped forward: had the late observation dragged the clock back to
    // T0+10, the next one would have measured its gap from there and split a
    // sitting that never went idle.
    assert_eq!(clock(&pool, user).await, Some(T0 + 800));

    observe(&pool, user, 30, T0 + 800 + IDLE_GAP_SECS).await;
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 20)]);
}

#[tokio::test]
async fn observe_percent_baselines_a_row_carrying_no_sitting_clock() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // What migration 0092 leaves behind if its backfill found no `updated_at`
    // to copy: a mark with no sitting in progress, which re-baselines rather
    // than differencing against a value of unknown vintage.
    sqlx::query(
        "INSERT INTO reading_progress_marks
             (user_id, book_uuid, format, sitting_max_percent, sitting_observed_at, updated_at)
         VALUES (?, ?, 'epub', 40, NULL, ?)",
    )
    .bind(user)
    .bind(UUID)
    .bind(T0)
    .execute(&pool)
    .await
    .unwrap();

    observe(&pool, user, 50, T0 + 60).await;

    assert!(days(&pool, user).await.is_empty());
    assert_eq!(mark(&pool, user).await, Some(50));
    assert_eq!(clock(&pool, user).await, Some(T0 + 60));
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
