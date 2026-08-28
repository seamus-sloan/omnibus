//! Unit tests for `db::stats::goals`: the read/write happy paths, every
//! `GoalError` variant, the per-year isolation AC7 turns on, and the cache
//! invalidation a just-saved goal depends on.

use omnibus_shared::{StatsRange, GOAL_KIND_BOOKS, MAX_GOAL_TARGET, MAX_GOAL_YEAR, MIN_GOAL_YEAR};

use super::*;
use crate::init_db;
use crate::test_support::seed_minimal_books;

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

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
    let year = current_year(&pool).await.unwrap();
    let when = early_in(&pool, year).await;
    finish(&pool, user, "uuid-1", when).await;
    finish(&pool, user, "uuid-2", when).await;

    let saved = set_goal(&pool, user, &ReadingGoalUpdate::books(24))
        .await
        .unwrap()
        .expect("a target was set, so a goal comes back");
    assert_eq!(saved.kind, GOAL_KIND_BOOKS);
    assert_eq!(saved.target, 24);
    assert_eq!(saved.current, 2);
    assert_eq!(saved.year, year);

    // And it is durable: an independent read sees the same thing.
    let read = goal_for_year(&pool, user, year).await.unwrap().unwrap();
    assert_eq!(read, saved);
}

#[tokio::test]
async fn set_goal_overwrites_an_existing_target_for_the_same_year() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    set_goal(&pool, user, &ReadingGoalUpdate::books(12))
        .await
        .unwrap();
    let raised = set_goal(&pool, user, &ReadingGoalUpdate::books(40))
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
    set_goal(&pool, user, &ReadingGoalUpdate::books(12))
        .await
        .unwrap();

    let cleared = set_goal(&pool, user, &ReadingGoalUpdate::clear_books())
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
    let err = set_goal(&pool, user, &update).await.unwrap_err();
    assert!(matches!(err, GoalError::UnsupportedKind(k) if k == "pages"));
}

#[tokio::test]
async fn set_goal_returns_invalid_target_outside_one_to_the_maximum() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    for bad in [0, -3, MAX_GOAL_TARGET + 1] {
        let err = set_goal(&pool, user, &ReadingGoalUpdate::books(bad))
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
        let err = set_goal(&pool, user, &update).await.unwrap_err();
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

    let err = set_goal(&pool, user, &ReadingGoalUpdate::books(12))
        .await
        .unwrap_err();
    assert!(matches!(err, GoalError::Sqlx(_)), "got {err:?}");
}

#[tokio::test]
async fn goal_for_year_is_none_when_the_user_has_set_nothing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool).await.unwrap();
    assert!(goal_for_year(&pool, user, year).await.unwrap().is_none());
}

#[tokio::test]
async fn goal_for_year_counts_only_completions_inside_that_year() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 4).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool).await.unwrap();

    // Two finished this year, one finished two years ago.
    finish(&pool, user, "uuid-1", early_in(&pool, year).await).await;
    finish(&pool, user, "uuid-2", early_in(&pool, year).await).await;
    finish(&pool, user, "uuid-3", early_in(&pool, year - 2).await).await;

    // A goal on each of the two years, so both are readable.
    set_goal(&pool, user, &ReadingGoalUpdate::books(24))
        .await
        .unwrap();
    let past = ReadingGoalUpdate {
        year: Some(year - 2),
        target: Some(5),
        ..Default::default()
    };
    set_goal(&pool, user, &past).await.unwrap();

    let this_year = goal_for_year(&pool, user, year).await.unwrap().unwrap();
    assert_eq!(this_year.current, 2);

    // AC7: filing a goal against a past year reports that year's real total
    // and leaves this year's alone.
    let then = goal_for_year(&pool, user, year - 2).await.unwrap().unwrap();
    assert_eq!(then.current, 1);
    assert_eq!(then.target, 5);
    assert_eq!(
        goal_for_year(&pool, user, year).await.unwrap().unwrap(),
        this_year
    );
}

#[tokio::test]
async fn goal_for_year_ignores_another_users_goal_and_completions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let year = current_year(&pool).await.unwrap();
    finish(&pool, bob, "uuid-1", early_in(&pool, year).await).await;

    set_goal(&pool, alice, &ReadingGoalUpdate::books(10))
        .await
        .unwrap();
    let alices = goal_for_year(&pool, alice, year).await.unwrap().unwrap();
    assert_eq!(alices.current, 0);
    assert!(goal_for_year(&pool, bob, year).await.unwrap().is_none());
}

#[tokio::test]
async fn goal_for_year_ignores_a_completion_on_a_book_that_no_longer_exists() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let year = current_year(&pool).await.unwrap();
    // Same liveness filter every other completion metric applies.
    finish(&pool, user, "uuid-ghost", early_in(&pool, year).await).await;
    set_goal(&pool, user, &ReadingGoalUpdate::books(10))
        .await
        .unwrap();

    assert_eq!(
        goal_for_year(&pool, user, year)
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
    let year = current_year(&pool).await.unwrap();
    finish(&pool, user, "uuid-1", early_in(&pool, year).await).await;
    set_goal(&pool, user, &ReadingGoalUpdate::books(24))
        .await
        .unwrap();

    // AC3: the goal is annual, so it reads identically on every range even
    // though a January-2nd completion falls outside the Week window.
    let mut seen = Vec::new();
    for range in StatsRange::ALL {
        let summary = compute::compute(&pool, user, range).await.unwrap();
        seen.push(summary.goal.expect("every range carries the goal"));
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "{seen:?}");
    assert_eq!(seen[0].target, 24);
    assert_eq!(seen[0].current, 1);
    assert_eq!(seen[0].year, year);
}

/// Seed a user with an explicit id. The stats cache is a process-wide static
/// keyed on `(user_id, range)` and every test pool restarts ids at 1, so a
/// test exercising the *cached* `user_stats` entry point claims an id no
/// sibling test can collide with — a `clear_cache()` here would race the
/// sibling TTL test rather than help.
async fn seed_user_with_id(pool: &SqlitePool, id: i64, name: &str) -> i64 {
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, '!x')")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn set_goal_invalidates_the_cached_summary_so_the_next_read_is_current() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user_with_id(&pool, 9931, "goal-cache").await;

    // Warm the cache with the no-goal answer, well inside the TTL.
    let before = super::super::user_stats(&pool, user, StatsRange::Week)
        .await
        .unwrap();
    assert!(before.goal.is_none());

    set_goal(&pool, user, &ReadingGoalUpdate::books(24))
        .await
        .unwrap();

    // AC5: without the invalidation this would still be the cached `None`
    // for up to STATS_TTL_SECS.
    let after = super::super::user_stats(&pool, user, StatsRange::Week)
        .await
        .unwrap();
    assert_eq!(after.goal.map(|g| g.target), Some(24));
}
