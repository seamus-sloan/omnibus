//! Unit tests for the forward-progress ledger: baselining, forward accrual,
//! backward moves, and slot bucketing.

use super::*;
use crate::init_db;
use crate::test_support::seed_user;

/// 2023-11-14 22:13:20 UTC.
const T0: i64 = 1_700_000_000;
const DAY: i64 = 86_400;

const UUID: &str = "uuid-a";

/// The whole ledger for one book rolled up to days, as `(day, percent_gained)`
/// ascending, read back at `offset_minutes`.
///
/// The rollup lives here rather than in the ledger because that is now the whole
/// design: storage keeps a quarter-hour, and *which day* that is stays a
/// question the reader's offset answers at read time.
async fn days_at(pool: &SqlitePool, user: i64, offset_minutes: i64) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT date(slot * 900 + ? * 60, 'unixepoch') AS day,
                SUM(percent_gained) AS gained
         FROM reading_progress_slots
         WHERE user_id = ? GROUP BY day ORDER BY day",
    )
    .bind(offset_minutes)
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// [`days_at`] in UTC — what the ledger used to store outright.
async fn days(pool: &SqlitePool, user: i64) -> Vec<(String, i64)> {
    days_at(pool, user, 0).await
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
async fn observe_percent_buckets_each_gain_into_its_own_day() {
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
async fn a_stored_gain_rebuckets_to_the_day_the_reader_is_on() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // 22:13 UTC — an evening read west of UTC, already tomorrow east of it.
    observe(&pool, user, 0, T0).await;
    observe(&pool, user, 20, T0 + 60).await;

    // The point of migration 0093: one stored gain, three calendars, no
    // rewrite. Storing a day string could only ever have answered the first.
    assert_eq!(
        days_at(&pool, user, 0).await,
        vec![("2023-11-14".into(), 20)]
    );
    assert_eq!(
        days_at(&pool, user, -420).await,
        vec![("2023-11-14".into(), 20)],
        "still the 14th in Los Angeles"
    );
    assert_eq!(
        days_at(&pool, user, 540).await,
        vec![("2023-11-15".into(), 20)],
        "already the 15th in Tokyo"
    );
}

#[tokio::test]
async fn a_gain_lands_in_the_quarter_hour_it_was_observed_in() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // Two observations 20 minutes apart straddle a quarter-hour boundary, so
    // they take separate rows — the granularity that makes a quarter-hour zone
    // (UTC+05:45) re-bucketable at all.
    observe(&pool, user, 0, T0).await;
    observe(&pool, user, 10, T0 + 60).await;
    observe(&pool, user, 25, T0 + 1_200).await;

    let slots: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT slot, percent_gained FROM reading_progress_slots
         WHERE user_id = ? ORDER BY slot",
    )
    .bind(user)
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(slots.len(), 2, "got {slots:?}");
    assert_eq!(slots[0], ((T0 + 60) / 900, 10));
    assert_eq!(slots[1], ((T0 + 1_200) / 900, 15));
}

#[tokio::test]
async fn a_gain_observed_before_the_epoch_floors_into_the_earlier_slot() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // A device with a badly wrong clock files one. Rust's `/` truncates toward
    // zero, which would round a negative instant *up* into the slot after the
    // one it happened in.
    observe(&pool, user, 0, -1_000).await;
    observe(&pool, user, 10, -900).await;

    let slot: i64 = sqlx::query_scalar("SELECT slot FROM reading_progress_slots WHERE user_id = ?")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        slot, -1,
        "-900s is the quarter-hour before the epoch, not 0"
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
