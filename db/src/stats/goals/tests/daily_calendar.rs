//! Which day the daily figures are cut on: the asking client's offset
//! (falling back to the most recent session's), pages from the ledger and
//! minutes from both session tables on that day, the label agreeing with
//! the figure, partial minutes truncated, offset-less sessions counted,
//! and per-reader scoping.

use omnibus_shared::StatsRange;

use super::super::*;
use super::{accrue, seed_user, set_pages, utc_day};
use crate::init_db;
use crate::test_support::seed_minimal_books;

/// A UTC+13 offset, in minutes. Chosen so a session at the start of the
/// reader's local day sits in the *previous* UTC day: local midnight minus 13
/// hours is 11:00 the day before. Every assertion about local-versus-UTC
/// bucketing below turns on that, and it holds whatever time the suite runs at.
const OFFSET_PLUS_13: i64 = 780;

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
