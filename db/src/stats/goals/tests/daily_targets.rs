//! The daily pages and minutes targets: `set_daily_goal` storing each
//! kind independently, overwriting, clearing one kind, leaving the annual
//! row alone, its kind and per-kind maximum validation, the figures
//! reported with and without a target, and the cache invalidation on save.

use omnibus_shared::{StatsRange, GOAL_KIND_BOOKS, MAX_DAILY_MINUTES, MAX_DAILY_PAGES};

use super::super::*;
use super::{accrue, seed_user, seed_user_with_id, set_pages, utc_day};
use crate::init_db;
use crate::test_support::seed_minimal_books;

#[tokio::test]
async fn set_daily_goal_stores_both_kinds_independently_and_reads_them_back() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let after_pages = set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 30),
        None,
    )
    .await
    .unwrap();
    assert_eq!(after_pages.pages.as_ref().map(|g| g.target), Some(30));
    assert!(after_pages.minutes.is_none());

    // Setting the second kind must leave the first exactly as it was.
    let both = set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();
    assert_eq!(both.pages.as_ref().map(|g| g.target), Some(30));
    assert_eq!(both.minutes.as_ref().map(|g| g.target), Some(20));
    assert_eq!(daily_goals(&pool, user, None).await.unwrap(), both);
}

#[tokio::test]
async fn set_daily_goal_overwrites_the_same_kind_rather_than_adding_a_second_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 30),
        None,
    )
    .await
    .unwrap();
    let raised = set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 50),
        None,
    )
    .await
    .unwrap();
    assert_eq!(raised.pages.map(|g| g.target), Some(50));

    // The partial unique index is what makes this a change and not a duplicate
    // — a plain UNIQUE over the nullable `year` would have let both rows in.
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_goals WHERE user_id = ? AND scope = 'day'",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn set_daily_goal_with_no_target_clears_only_that_kind() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 30),
        None,
    )
    .await
    .unwrap();
    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();

    let left = set_daily_goal(&pool, user, &DailyGoalUpdate::clear(GOAL_KIND_PAGES), None)
        .await
        .unwrap();
    assert!(left.pages.is_none(), "the cleared kind is gone");
    assert_eq!(
        left.minutes.map(|g| g.target),
        Some(20),
        "the other kind is untouched"
    );
}

#[tokio::test]
async fn set_daily_goal_does_not_disturb_the_annual_goal_in_the_same_table() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    set_goal(&pool, user, &ReadingGoalUpdate::books(24), None)
        .await
        .unwrap();

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 30),
        None,
    )
    .await
    .unwrap();
    let year = current_year(&pool, 0).await.unwrap();
    assert_eq!(
        goal_for_year(&pool, user, year, None)
            .await
            .unwrap()
            .unwrap()
            .target,
        24
    );

    // And clearing the annual one leaves the daily one standing.
    set_goal(&pool, user, &ReadingGoalUpdate::clear_books(), None)
        .await
        .unwrap();
    assert_eq!(
        daily_goals(&pool, user, None)
            .await
            .unwrap()
            .pages
            .map(|g| g.target),
        Some(30)
    );
}

#[tokio::test]
async fn set_daily_goal_returns_unsupported_kind_for_a_kind_with_no_daily_measurement() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    for bad in [GOAL_KIND_BOOKS, "hours"] {
        let err = set_daily_goal(&pool, user, &DailyGoalUpdate::set(bad, 5), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, GoalError::UnsupportedKind(ref k) if k == bad),
            "expected UnsupportedKind for {bad}, got {err:?}"
        );
    }
}

#[tokio::test]
async fn set_daily_goal_returns_invalid_daily_target_against_the_per_kind_maximum() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    for (kind, max) in [
        (GOAL_KIND_PAGES, MAX_DAILY_PAGES),
        (GOAL_KIND_MINUTES, MAX_DAILY_MINUTES),
    ] {
        for bad in [0, -3, max + 1] {
            let err = set_daily_goal(&pool, user, &DailyGoalUpdate::set(kind, bad), None)
                .await
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    GoalError::InvalidDailyTarget { kind: ref k, max: m, got }
                        if k == kind && m == max && got == bad
                ),
                "expected InvalidDailyTarget for {kind} {bad}, got {err:?}"
            );
        }
    }

    // The bound really is per-kind: a target legal as pages is not as minutes.
    assert!(set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 1_500),
        None
    )
    .await
    .is_ok());
    assert!(set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 1_500),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn set_daily_goal_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    pool.close().await;

    let err = set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 30),
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, GoalError::Sqlx(_)), "got {err:?}");
}

#[tokio::test]
async fn daily_goals_still_reports_todays_figures_when_no_target_is_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    set_pages(&pool, "uuid-1", 300).await;
    let today = utc_day(&pool).await;
    accrue(&pool, user, "uuid-1", &today, 10).await;

    let goals = daily_goals(&pool, user, None).await.unwrap();
    // No goal is still no goal — the surfaces gate their rings on this.
    assert!(goals.is_empty());
    assert!(goals.pages.is_none() && goals.minutes.is_none());
    assert_eq!(goals.unzoned_seconds, 0, "nothing to disclose against");
    // But the day's figures are measured anyway, so a surface can show what
    // the reader has done before they commit to a target.
    assert_eq!(goals.pages_today, Some(30), "10% of a 300-page book");
    assert_eq!(goals.minutes_today, Some(0));
}

/// The figure must not move the moment a target is set: `current` and the
/// `*_today` field are the same measurement, computed once and shared.
#[tokio::test]
async fn daily_goals_report_the_same_figure_with_and_without_a_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    set_pages(&pool, "uuid-1", 300).await;
    let today = utc_day(&pool).await;
    accrue(&pool, user, "uuid-1", &today, 10).await;

    let before = daily_goals(&pool, user, None).await.unwrap();
    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 50),
        None,
    )
    .await
    .unwrap();
    let after = daily_goals(&pool, user, None).await.unwrap();

    assert_eq!(before.pages_today, after.pages_today);
    assert_eq!(
        after.pages.as_ref().map(|g| g.current),
        after.pages_today,
        "the goal's progress and the bare figure are one number"
    );
}

#[tokio::test]
async fn set_daily_goal_invalidates_the_cached_summary_so_the_next_read_is_current() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user_with_id(&pool, 9932, "daily-goal-cache").await;

    let before = super::super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert!(before.daily_goals.is_empty());

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();

    let after = super::super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert_eq!(after.daily_goals.minutes.map(|g| g.target), Some(20));
}
