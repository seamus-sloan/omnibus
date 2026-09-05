//! The annual books goal: `set_goal` storing, overwriting and clearing a
//! target with its kind/target/year validation, `goal_for_year` counting
//! only that year's completions for that user, the goal riding every
//! summary, and the cache invalidation on save.

use omnibus_shared::{StatsRange, GOAL_KIND_BOOKS, MAX_GOAL_TARGET, MAX_GOAL_YEAR, MIN_GOAL_YEAR};

use super::super::*;
use super::{seed_user, seed_user_with_id};
use crate::init_db;
use crate::test_support::seed_minimal_books;

/// Stamp an explicit read-status `finished` on a book at `finished_at` — one
/// of the two `FINISHED_EVENTS` sources, and the cheaper one to seed.
async fn finish(pool: &SqlitePool, user: i64, uuid: &str, finished_at: i64) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, 'finished', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(finished_at)
    .bind(finished_at)
    .execute(pool)
    .await
    .unwrap();
}

/// Unix seconds at noon on Jan 2nd of `year` — safely inside the year on
/// either side of a timezone, which the UTC-bounded count is not sensitive to
/// but a reader of this file might wonder about.
async fn early_in(pool: &SqlitePool, year: i64) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT CAST(strftime('%s', ? || '-01-02 12:00:00') AS INTEGER)")
        .bind(format!("{year:04}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn set_goal_stores_a_target_and_reads_back_with_progress() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool, 0).await.unwrap();
    let when = early_in(&pool, year).await;
    finish(&pool, user, "uuid-1", when).await;
    finish(&pool, user, "uuid-2", when).await;

    let saved = set_goal(&pool, user, &ReadingGoalUpdate::books(24), None)
        .await
        .unwrap()
        .expect("a target was set, so a goal comes back");
    assert_eq!(saved.kind, GOAL_KIND_BOOKS);
    assert_eq!(saved.target, 24);
    assert_eq!(saved.current, 2);
    assert_eq!(saved.year, year);

    // And it is durable: an independent read sees the same thing.
    let read = goal_for_year(&pool, user, year, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read, saved);
}

#[tokio::test]
async fn set_goal_overwrites_an_existing_target_for_the_same_year() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    set_goal(&pool, user, &ReadingGoalUpdate::books(12), None)
        .await
        .unwrap();
    let raised = set_goal(&pool, user, &ReadingGoalUpdate::books(40), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raised.target, 40);

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_goals WHERE user_id = ?")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "the unique key means a change, not a second row");
}

#[tokio::test]
async fn set_goal_with_no_target_clears_the_row_rather_than_storing_zero() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    set_goal(&pool, user, &ReadingGoalUpdate::books(12), None)
        .await
        .unwrap();

    let cleared = set_goal(&pool, user, &ReadingGoalUpdate::clear_books(), None)
        .await
        .unwrap();
    assert!(cleared.is_none());

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_goals WHERE user_id = ?")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn set_goal_returns_unsupported_kind_for_a_kind_other_than_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let update = ReadingGoalUpdate {
        kind: Some("pages".to_string()),
        target: Some(12),
        ..Default::default()
    };
    let err = set_goal(&pool, user, &update, None).await.unwrap_err();
    assert!(matches!(err, GoalError::UnsupportedKind(k) if k == "pages"));
}

#[tokio::test]
async fn set_goal_returns_invalid_target_outside_one_to_the_maximum() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    for bad in [0, -3, MAX_GOAL_TARGET + 1] {
        let err = set_goal(&pool, user, &ReadingGoalUpdate::books(bad), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, GoalError::InvalidTarget(t) if t == bad),
            "expected InvalidTarget for {bad}, got {err:?}"
        );
    }
}

#[tokio::test]
async fn set_goal_returns_invalid_year_outside_the_supported_range() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    for bad in [MIN_GOAL_YEAR - 1, MAX_GOAL_YEAR + 1] {
        let update = ReadingGoalUpdate {
            year: Some(bad),
            target: Some(12),
            ..Default::default()
        };
        let err = set_goal(&pool, user, &update, None).await.unwrap_err();
        assert!(
            matches!(err, GoalError::InvalidYear(y) if y == bad),
            "expected InvalidYear for {bad}, got {err:?}"
        );
    }
}

#[tokio::test]
async fn set_goal_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    pool.close().await;

    let err = set_goal(&pool, user, &ReadingGoalUpdate::books(12), None)
        .await
        .unwrap_err();
    assert!(matches!(err, GoalError::Sqlx(_)), "got {err:?}");
}

/// The year's count is reported whether or not a goal exists, and it is the
/// same number the goal's own `current` carries — setting a target must not
/// appear to move the count it measures.
#[tokio::test]
async fn current_goal_and_progress_counts_the_year_with_or_without_a_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool, 0).await.unwrap();
    let when = early_in(&pool, year).await;
    for uuid in ["uuid-1", "uuid-2"] {
        finish(&pool, user, uuid, when).await;
    }

    let (goal, count) = current_goal_and_progress(&pool, user, 0).await.unwrap();
    assert!(goal.is_none(), "no target set");
    assert_eq!(count, 2, "the year is counted anyway");

    set_goal(&pool, user, &ReadingGoalUpdate::books(30), None)
        .await
        .unwrap();
    let (goal, after) = current_goal_and_progress(&pool, user, 0).await.unwrap();
    assert_eq!(after, count, "the count did not move when the goal was set");
    assert_eq!(
        goal.map(|g| g.current),
        Some(count),
        "the goal's progress and the bare figure are one number"
    );
}

#[tokio::test]
async fn goal_for_year_is_none_when_the_user_has_set_nothing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool, 0).await.unwrap();
    assert!(goal_for_year(&pool, user, year, None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn goal_for_year_counts_only_completions_inside_that_year() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 4).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool, 0).await.unwrap();

    // Two finished this year, one finished two years ago.
    finish(&pool, user, "uuid-1", early_in(&pool, year).await).await;
    finish(&pool, user, "uuid-2", early_in(&pool, year).await).await;
    finish(&pool, user, "uuid-3", early_in(&pool, year - 2).await).await;

    // A goal on each of the two years, so both are readable.
    set_goal(&pool, user, &ReadingGoalUpdate::books(24), None)
        .await
        .unwrap();
    let past = ReadingGoalUpdate {
        year: Some(year - 2),
        target: Some(5),
        ..Default::default()
    };
    set_goal(&pool, user, &past, None).await.unwrap();

    let this_year = goal_for_year(&pool, user, year, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(this_year.current, 2);

    // AC7: filing a goal against a past year reports that year's real total
    // and leaves this year's alone.
    let then = goal_for_year(&pool, user, year - 2, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(then.current, 1);
    assert_eq!(then.target, 5);
    assert_eq!(
        goal_for_year(&pool, user, year, None)
            .await
            .unwrap()
            .unwrap(),
        this_year
    );
}

#[tokio::test]
async fn goal_for_year_ignores_another_users_goal_and_completions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let year = current_year(&pool, 0).await.unwrap();
    finish(&pool, bob, "uuid-1", early_in(&pool, year).await).await;

    set_goal(&pool, alice, &ReadingGoalUpdate::books(10), None)
        .await
        .unwrap();
    let alices = goal_for_year(&pool, alice, year, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alices.current, 0);
    assert!(goal_for_year(&pool, bob, year, None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn goal_for_year_ignores_a_completion_on_a_book_that_no_longer_exists() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool, 0).await.unwrap();
    // Same liveness filter every other completion metric applies.
    finish(&pool, user, "uuid-ghost", early_in(&pool, year).await).await;
    set_goal(&pool, user, &ReadingGoalUpdate::books(10), None)
        .await
        .unwrap();

    assert_eq!(
        goal_for_year(&pool, user, year, None)
            .await
            .unwrap()
            .unwrap()
            .current,
        0
    );
}

#[tokio::test]
async fn current_goal_rides_every_summary_and_does_not_move_with_the_range() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool, 0).await.unwrap();
    finish(&pool, user, "uuid-1", early_in(&pool, year).await).await;
    set_goal(&pool, user, &ReadingGoalUpdate::books(24), None)
        .await
        .unwrap();

    // AC3: the goal is annual, so it reads identically on every range even
    // though a January-2nd completion falls outside the Week window.
    let mut seen = Vec::new();
    for range in StatsRange::ALL {
        let summary = compute::compute(&pool, user, range, 0).await.unwrap();
        seen.push(summary.goal.expect("every range carries the goal"));
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "{seen:?}");
    assert_eq!(seen[0].target, 24);
    assert_eq!(seen[0].current, 1);
    assert_eq!(seen[0].year, year);
}

#[tokio::test]
async fn set_goal_invalidates_the_cached_summary_so_the_next_read_is_current() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user_with_id(&pool, 9931, "goal-cache").await;

    // Warm the cache with the no-goal answer, well inside the TTL.
    let before = super::super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert!(before.goal.is_none());

    set_goal(&pool, user, &ReadingGoalUpdate::books(24), None)
        .await
        .unwrap();

    // AC5: without the invalidation this would still be the cached `None`
    // for up to STATS_TTL_SECS.
    let after = super::super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert_eq!(after.goal.map(|g| g.target), Some(24));
}
