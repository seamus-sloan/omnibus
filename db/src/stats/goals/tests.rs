//! Unit tests for `db::stats::goals`: the read/write happy paths for both
//! scopes, every `GoalError` variant, the per-year isolation AC7 turns on, the
//! local-versus-UTC day the two daily kinds are measured over and its agreement
//! with the day they label, and the cache invalidation a just-saved goal depends
//! on.

use omnibus_shared::{
    StatsRange, GOAL_KIND_BOOKS, MAX_DAILY_MINUTES, MAX_DAILY_PAGES, MAX_GOAL_TARGET,
    MAX_GOAL_YEAR, MIN_GOAL_YEAR,
};

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
    let before = super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert!(before.goal.is_none());

    set_goal(&pool, user, &ReadingGoalUpdate::books(24), None)
        .await
        .unwrap();

    // AC5: without the invalidation this would still be the cached `None`
    // for up to STATS_TTL_SECS.
    let after = super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert_eq!(after.goal.map(|g| g.target), Some(24));
}

// --- Daily goals -----------------------------------------------------------

/// A UTC+13 offset, in minutes. Chosen so a session at the start of the
/// reader's local day sits in the *previous* UTC day: local midnight minus 13
/// hours is 11:00 the day before. Every assertion about local-versus-UTC
/// bucketing below turns on that, and it holds whatever time the suite runs at.
const OFFSET_PLUS_13: i64 = 780;

/// Give a seeded book an exact page count — rung 2 of the length ladder, and
/// the cheapest to seed without inventing a word count.
async fn set_pages(pool: &SqlitePool, uuid: &str, pages: i64) {
    sqlx::query("UPDATE books SET page_count = ? WHERE uuid = ?")
        .bind(pages)
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

/// Accrue a day's forward progress in the `0083` ledger, as the progress write
/// path does. `day` is UTC `YYYY-MM-DD`, which is the only calendar this table
/// has.
async fn accrue(pool: &SqlitePool, user: i64, uuid: &str, day: &str, percent: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(percent)
    .execute(pool)
    .await
    .unwrap();
}

/// File a reading session of `secs` starting at `started_at`, carrying an
/// optional capture-time offset — `None` is the pre-`0080` row the local-day
/// rollup cannot place.
async fn read_session(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
    offset: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO reading_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_read, utc_offset_minutes)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .bind(offset)
    .execute(pool)
    .await
    .unwrap();
}

/// The same for a listening session, so the minutes goal can be shown to count
/// both.
async fn listen_session(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
    offset: i64,
) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened, utc_offset_minutes)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .bind(offset)
    .execute(pool)
    .await
    .unwrap();
}

/// Today's UTC date, `YYYY-MM-DD`.
async fn utc_day(pool: &SqlitePool) -> String {
    sqlx::query_scalar("SELECT date('now')")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The unix second `secs_into_day` seconds after local midnight, for a reader
/// at `offset` minutes east. Resolved in SQLite so it lands on the same
/// calendar the rollup reads.
async fn local_day_start_plus(pool: &SqlitePool, offset: i64, secs_into_day: i64) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT CAST(strftime('%s', date(strftime('%s','now') + ? * 60, 'unixepoch')
                               || ' 00:00:00') AS INTEGER) - ? * 60 + ?",
    )
    .bind(offset)
    .bind(offset)
    .bind(secs_into_day)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The reader's local day for a given offset, `YYYY-MM-DD`.
async fn local_day(pool: &SqlitePool, offset: i64) -> String {
    sqlx::query_scalar("SELECT date(strftime('%s','now') + ? * 60, 'unixepoch')")
        .bind(offset)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The server's current unix second.
async fn now_secs(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT CAST(strftime('%s','now') AS INTEGER)")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn daily_goals_take_the_offset_from_the_most_recent_session_in_either_table() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let now = now_secs(&pool).await;

    // A reader who has moved: one session at UTC-12 and one at UTC+13. The two
    // are 25 hours apart, so they can never name the same local day — which is
    // what makes the reported `day` say which offset won.
    const WEST: i64 = -720;
    let west_day = local_day(&pool, WEST).await;
    let east_day = local_day(&pool, OFFSET_PLUS_13).await;
    assert_ne!(west_day, east_day, "the fixture must separate the two days");

    // Alice's newer session is the *listening* one.
    let alice = seed_user(&pool, "alice").await;
    read_session(&pool, alice, "uuid-1", now - 3_600, 60, Some(WEST)).await;
    listen_session(&pool, alice, "uuid-1", now - 60, 60, OFFSET_PLUS_13).await;
    set_daily_goal(
        &pool,
        alice,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        daily_goals(&pool, alice, None)
            .await
            .unwrap()
            .minutes
            .unwrap()
            .day,
        east_day
    );

    // Bob's is the *reading* one, so a probe that simply preferred one table
    // over the other would answer one of these two wrongly.
    let bob = seed_user(&pool, "bob").await;
    listen_session(&pool, bob, "uuid-1", now - 3_600, 60, OFFSET_PLUS_13).await;
    read_session(&pool, bob, "uuid-1", now - 60, 60, Some(WEST)).await;
    set_daily_goal(
        &pool,
        bob,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        daily_goals(&pool, bob, None)
            .await
            .unwrap()
            .minutes
            .unwrap()
            .day,
        west_day
    );
}

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
async fn daily_goals_counts_pages_from_the_ledger_on_the_utc_day_only() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    set_pages(&pool, "uuid-1", 300).await;
    set_pages(&pool, "uuid-2", 100).await;
    let today = utc_day(&pool).await;
    let yesterday: String = sqlx::query_scalar("SELECT date('now', '-1 day')")
        .fetch_one(&pool)
        .await
        .unwrap();

    // 10% of a 300-page book plus 25% of a 100-page one = 30 + 25.
    accrue(&pool, user, "uuid-1", &today, 10).await;
    accrue(&pool, user, "uuid-2", &today, 25).await;
    accrue(&pool, user, "uuid-1", &yesterday, 50).await;

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 40),
        None,
    )
    .await
    .unwrap();
    let goal = daily_goals(&pool, user, None).await.unwrap().pages.unwrap();
    assert_eq!(goal.current, 55, "yesterday's 150 pages are not today's");
    assert_eq!(goal.day, today);
    assert!(goal.is_met());
    assert_eq!(goal.remaining(), 0);
}

#[tokio::test]
async fn daily_goals_counts_minutes_on_the_readers_local_day_not_the_utc_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // At UTC+13 the reader's day opens at 11:00 the previous UTC day, so these
    // two sessions share a UTC day and straddle a *local* one. A UTC rollup
    // would count both; the local one must count only the first.
    let just_after_local_midnight = local_day_start_plus(&pool, OFFSET_PLUS_13, 30).await;
    let just_before_local_midnight = local_day_start_plus(&pool, OFFSET_PLUS_13, -30).await;
    read_session(
        &pool,
        user,
        "uuid-1",
        just_after_local_midnight,
        600,
        Some(OFFSET_PLUS_13),
    )
    .await;
    read_session(
        &pool,
        user,
        "uuid-1",
        just_before_local_midnight,
        1_800,
        Some(OFFSET_PLUS_13),
    )
    .await;

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();
    let goal = daily_goals(&pool, user, None)
        .await
        .unwrap()
        .minutes
        .unwrap();
    assert_eq!(goal.current, 10, "only the session on the local day counts");
    assert!(!goal.is_met());
    assert_eq!(goal.remaining(), 10);

    // The two really did share a UTC day — otherwise the assertion above would
    // pass for a UTC implementation too, and prove nothing.
    let same_utc_day: bool =
        sqlx::query_scalar("SELECT date(?, 'unixepoch') = date(?, 'unixepoch')")
            .bind(just_after_local_midnight)
            .bind(just_before_local_midnight)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(same_utc_day, "the fixture must straddle only the local day");
}

/// The day is an argument, not a second clock read: a request that crossed
/// local midnight between resolving the label and measuring the figure would
/// otherwise report one day's date against the next day's minutes.
#[tokio::test]
async fn day_seconds_measures_the_day_it_is_handed_rather_than_re_reading_the_clock() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    let midday = local_day_start_plus(&pool, OFFSET_PLUS_13, 43_200).await;
    read_session(&pool, user, "uuid-1", midday, 600, Some(OFFSET_PLUS_13)).await;
    read_session(
        &pool,
        user,
        "uuid-1",
        midday - 86_400,
        1_800,
        Some(OFFSET_PLUS_13),
    )
    .await;

    let today = local_day(&pool, OFFSET_PLUS_13).await;
    let yesterday: String = sqlx::query_scalar("SELECT date(?, '-1 day')")
        .bind(&today)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        day_seconds(&pool, user, &today, OFFSET_PLUS_13)
            .await
            .unwrap(),
        600
    );
    assert_eq!(
        day_seconds(&pool, user, &yesterday, OFFSET_PLUS_13)
            .await
            .unwrap(),
        1_800,
        "the day asked for decides the figure, not `now`"
    );
}

/// The reported `day` and the figure beside it describe the same day, for both
/// kinds — the label is resolved once and every measurement is taken against it.
#[tokio::test]
async fn daily_goals_label_and_figure_name_the_same_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    let midday = local_day_start_plus(&pool, OFFSET_PLUS_13, 43_200).await;
    read_session(&pool, user, "uuid-1", midday, 600, Some(OFFSET_PLUS_13)).await;
    read_session(
        &pool,
        user,
        "uuid-1",
        midday - 86_400,
        1_800,
        Some(OFFSET_PLUS_13),
    )
    .await;
    for kind in [GOAL_KIND_PAGES, GOAL_KIND_MINUTES] {
        set_daily_goal(
            &pool,
            user,
            &DailyGoalUpdate::set(kind, 20),
            Some(OFFSET_PLUS_13),
        )
        .await
        .unwrap();
    }

    let goals = daily_goals(&pool, user, Some(OFFSET_PLUS_13))
        .await
        .unwrap();
    let minutes = goals.minutes.unwrap();
    assert_eq!(minutes.day, local_day(&pool, OFFSET_PLUS_13).await);
    assert_eq!(
        minutes.current, 10,
        "the previous local day's half hour belongs to that day's label"
    );
    assert_eq!(
        goals.pages.unwrap().day,
        minutes.day,
        "both kinds carry the one label"
    );
}

#[tokio::test]
async fn daily_goals_counts_listening_toward_the_minutes_goal_alongside_reading() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let midday = local_day_start_plus(&pool, OFFSET_PLUS_13, 43_200).await;

    read_session(&pool, user, "uuid-1", midday, 600, Some(OFFSET_PLUS_13)).await;
    listen_session(&pool, user, "uuid-1", midday, 900, OFFSET_PLUS_13).await;

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 30),
        None,
    )
    .await
    .unwrap();
    let goal = daily_goals(&pool, user, None)
        .await
        .unwrap()
        .minutes
        .unwrap();
    assert_eq!(goal.current, 25, "10 read + 15 listened");
}

#[tokio::test]
async fn daily_goals_truncates_a_partial_minute_rather_than_rounding_it_up() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let midday = local_day_start_plus(&pool, OFFSET_PLUS_13, 43_200).await;
    read_session(&pool, user, "uuid-1", midday, 59, Some(OFFSET_PLUS_13)).await;

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 1),
        None,
    )
    .await
    .unwrap();
    let goal = daily_goals(&pool, user, None)
        .await
        .unwrap()
        .minutes
        .unwrap();
    assert_eq!(goal.current, 0, "59 seconds is not a minute read");
    assert!(!goal.is_met());
}

#[tokio::test]
async fn daily_goals_counts_a_session_that_recorded_no_offset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let midday = local_day_start_plus(&pool, OFFSET_PLUS_13, 43_200).await;
    read_session(&pool, user, "uuid-1", midday, 600, Some(OFFSET_PLUS_13)).await;
    // A pre-0080 row. It used to be unplaceable — the day came from each
    // session's *own* captured offset, and this one has none — so it was
    // excluded from the goal and disclosed separately. The day now comes from
    // the reader's current offset instead, which `started_at` alone can be
    // measured against, so there is nothing left that cannot be placed.
    read_session(&pool, user, "uuid-1", midday + 60, 300, None).await;

    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 30),
        Some(OFFSET_PLUS_13),
    )
    .await
    .unwrap();
    let goals = daily_goals(&pool, user, Some(OFFSET_PLUS_13))
        .await
        .unwrap();

    assert_eq!(
        goals.minutes.unwrap().current,
        15,
        "both sessions count toward the reader's day"
    );
    assert_eq!(
        goals.unzoned_seconds, 0,
        "nothing is excluded, so there is nothing to disclose"
    );
}

#[tokio::test]
async fn daily_goals_answer_the_asking_clients_day_not_the_stored_offsets() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // A minute past midnight on the UTC+13 day, recorded with that offset — but
    // the reader has since moved, and the client asking is 12 hours west of UTC.
    // Just-past-midnight rather than midday on purpose: the two zones are 25
    // hours apart, so only a sitting near a boundary is guaranteed to fall on
    // different days for them.
    let just_after_midnight = local_day_start_plus(&pool, OFFSET_PLUS_13, 60).await;
    read_session(
        &pool,
        user,
        "uuid-1",
        just_after_midnight,
        600,
        Some(OFFSET_PLUS_13),
    )
    .await;
    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 30),
        Some(OFFSET_PLUS_13),
    )
    .await
    .unwrap();

    let there = daily_goals(&pool, user, Some(OFFSET_PLUS_13))
        .await
        .unwrap();
    let here = daily_goals(&pool, user, Some(-12 * 60)).await.unwrap();

    // The client's claim decides, not the offset the session recorded — that is
    // the single calendar, and it is why the figure is not a property of the
    // stored rows alone.
    assert_eq!(there.minutes_today, Some(10));
    assert_eq!(here.minutes_today, Some(0));
    assert_ne!(
        there.minutes.map(|g| g.day),
        here.minutes.map(|g| g.day),
        "the two clients are not on the same day"
    );
}

#[tokio::test]
async fn daily_goals_ignores_another_readers_sessions_and_ledger_rows() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    set_pages(&pool, "uuid-1", 300).await;
    let today = utc_day(&pool).await;
    let midday = local_day_start_plus(&pool, OFFSET_PLUS_13, 43_200).await;

    accrue(&pool, bob, "uuid-1", &today, 50).await;
    read_session(&pool, bob, "uuid-1", midday, 3_600, Some(OFFSET_PLUS_13)).await;

    set_daily_goal(
        &pool,
        alice,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 30),
        None,
    )
    .await
    .unwrap();
    set_daily_goal(
        &pool,
        alice,
        &DailyGoalUpdate::set(GOAL_KIND_MINUTES, 20),
        None,
    )
    .await
    .unwrap();
    let goals = daily_goals(&pool, alice, None).await.unwrap();
    assert_eq!(goals.pages.unwrap().current, 0);
    assert_eq!(goals.minutes.unwrap().current, 0);
    assert!(daily_goals(&pool, bob, None).await.unwrap().is_empty());
}

#[tokio::test]
async fn daily_goals_ride_every_summary_and_do_not_move_with_the_range() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    set_pages(&pool, "uuid-1", 300).await;
    accrue(&pool, user, "uuid-1", &utc_day(&pool).await, 10).await;
    set_daily_goal(
        &pool,
        user,
        &DailyGoalUpdate::set(GOAL_KIND_PAGES, 40),
        None,
    )
    .await
    .unwrap();

    let mut seen = Vec::new();
    for range in StatsRange::ALL {
        seen.push(
            compute::compute(&pool, user, range, 0)
                .await
                .unwrap()
                .daily_goals,
        );
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "{seen:?}");
    assert_eq!(seen[0].pages.as_ref().map(|g| g.current), Some(30));
}

#[tokio::test]
async fn set_daily_goal_invalidates_the_cached_summary_so_the_next_read_is_current() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user_with_id(&pool, 9932, "daily-goal-cache").await;

    let before = super::super::user_stats(&pool, user, StatsRange::Week, None)
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

    let after = super::super::user_stats(&pool, user, StatsRange::Week, None)
        .await
        .unwrap();
    assert_eq!(after.daily_goals.minutes.map(|g| g.target), Some(20));
}
