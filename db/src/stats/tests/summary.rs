//! The summary figures over seeded sessions: hours, sittings and active
//! days, the heatmap and biggest day on one calendar, streaks, `as_of`,
//! the busiest week, top authors and tags, daily listening, the empty
//! library, per-user scoping, the TTL cache, and the public entry point.

use omnibus_shared::StatsRange;

use super::super::*;
use super::{
    book_id, link_author, link_tag, listening_session, reading_session, seed_user, DAY, T0,
};
use crate::init_db;
use crate::test_support::seed_minimal_books;

#[tokio::test]
async fn all_time_aggregates_hours_sessions_and_active_days() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;

    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + DAY, 1200).await;
    listening_session(&pool, user, "uuid-1", T0 + 2 * DAY, 300).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

    assert_eq!(s.reading_seconds, 1800);
    assert_eq!(s.listening_seconds, 300);
    assert_eq!(s.total_seconds(), 2100);
    assert_eq!(s.sessions, 3);
    assert_eq!(s.active_days, 3);
    assert_eq!(s.heatmap.len(), 3);
    assert_eq!(s.heatmap.iter().map(|d| d.seconds).sum::<i64>(), 2100);
}

#[tokio::test]
async fn heatmap_and_biggest_day_name_the_same_day_for_a_reader_west_of_utc() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // UTC-7, one Monday: 10:00 local, then 20:00 local — which is 03:00 UTC
    // the next day. In UTC the evening sitting is a separate, larger day.
    const OFFSET: i64 = -420;
    const D: i64 = 19_675 * DAY; // 2023-11-14 00:00 UTC
    reading_session(&pool, user, "uuid-1", D - 7 * 3600, 3000).await;
    reading_session(&pool, user, "uuid-1", D + 3 * 3600, 3600).await;

    let s = compute(&pool, user, StatsRange::AllTime, OFFSET)
        .await
        .unwrap();

    // Both figures ride one summary onto one page, so a superlative naming a
    // day the heatmap draws as empty is a contradiction the reader can see.
    assert_eq!(s.heatmap.len(), 1);
    assert_eq!(s.heatmap[0].day, "2023-11-13");
    assert_eq!(s.heatmap[0].seconds, 6600);
    let biggest = s.superlatives.biggest_day.unwrap();
    assert_eq!(biggest.day, s.heatmap[0].day);
    assert_eq!(biggest.seconds, s.heatmap[0].seconds);
}

#[tokio::test]
async fn session_count_stitches_contiguous_checkpoint_rows_into_one_sitting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // An hour of web reading as sixty 60s heartbeat flushes.
    for i in 0..60 {
        reading_session(&pool, user, "uuid-1", T0 + i * 60, 60).await;
    }

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

    assert_eq!(s.sessions, 1);
    assert_eq!(s.reading_seconds, 3600, "the stitch must not lose time");
}

#[tokio::test]
async fn session_count_counts_two_books_read_back_to_back_separately() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    // No idle between them, but sittings are scoped per book — so this is
    // two pickups, and the user-wide figure stays the sum of each book's.
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-2", T0 + 600, 600).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

    assert_eq!(s.sessions, 2);
}

#[tokio::test]
async fn session_count_excludes_glances_but_keeps_their_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + DAY, 20).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

    assert_eq!(s.sessions, 1);
    assert_eq!(s.reading_seconds, 620);
}

#[tokio::test]
async fn streak_counts_longest_consecutive_run() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Days 0,1,2 (streak 3), gap, then day 5,6 (streak 2).
    for d in [0, 1, 2, 5, 6] {
        reading_session(&pool, user, "uuid-1", T0 + d * DAY, 60).await;
    }

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.active_days, 5);
    assert_eq!(s.longest_streak_days, 3);
    // T0 is years in the past, so the record stands but no run is live.
    assert_eq!(s.current_streak_days, 0);
}

#[tokio::test]
async fn as_of_returns_a_day_string_and_day_number_describing_the_same_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // The whole reason `as_of` reads both out of one statement: the heatmap
    // anchors on the string and the streak on the number, so a disagreement
    // between them silently moves the streak's anchor by a day.
    let (day, dnum) = as_of(&pool, 0).await.unwrap();
    let round_tripped: String = sqlx::query_scalar("SELECT date(? * 86400, 'unixepoch')")
        .bind(dnum)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(day.len(), 10);
    assert_eq!(round_tripped, day);
}

#[tokio::test]
async fn current_streak_runs_up_to_the_servers_today() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Anchored on the real clock rather than T0: the streak's "today" comes
    // from the server, so only a run reaching it is live. Seeded at midday so
    // the session can't slide into an adjacent day near a boundary.
    let today_noon = now_secs() / DAY * DAY + DAY / 2;
    for back in [2, 1, 0] {
        reading_session(&pool, user, "uuid-1", today_noon - back * DAY, 600).await;
    }

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

    assert_eq!(s.current_streak_days, 3);
    assert_eq!(s.longest_streak_days, 3);
}

#[tokio::test]
async fn busiest_week_picks_highest_seconds_week() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // 2023-11-14 (Tue) is well inside one ISO week; +14 days is two weeks later.
    reading_session(&pool, user, "uuid-1", T0, 100).await;
    reading_session(&pool, user, "uuid-1", T0 + 14 * DAY, 5000).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.busiest_week_seconds, 5000);
    // The winning session falls on Tue 2023-11-28; the field is the week's own
    // Monday, not the first day the reader was active in it, so that surfaces
    // can label it "Week of …" without naming a Tuesday as a week start.
    assert_eq!(s.busiest_week_start.as_deref(), Some("2023-11-27"));
}

#[tokio::test]
async fn busiest_week_start_is_the_weeks_monday_not_its_first_active_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // A reader who only read Wed/Thu of the week beginning Mon 2023-11-13.
    reading_session(&pool, user, "uuid-1", T0 + DAY, 1000).await;
    reading_session(&pool, user, "uuid-1", T0 + 2 * DAY, 1000).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.busiest_week_start.as_deref(), Some("2023-11-13"));
}

#[tokio::test]
async fn top_authors_and_tags_rank_by_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let b1 = book_id(&pool, "uuid-1").await;
    let b2 = book_id(&pool, "uuid-2").await;

    link_author(&pool, b1, "Ursula K. Le Guin").await;
    link_author(&pool, b2, "Isaac Asimov").await;
    link_tag(&pool, b1, "sci-fi").await;
    link_tag(&pool, b2, "sci-fi").await;
    link_tag(&pool, b2, "classic").await;

    reading_session(&pool, user, "uuid-1", T0, 900).await;
    reading_session(&pool, user, "uuid-2", T0, 300).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

    assert_eq!(s.top_authors[0].name, "Ursula K. Le Guin");
    assert_eq!(s.top_authors[0].seconds, 900);
    assert_eq!(s.top_authors[1].name, "Isaac Asimov");
    // sci-fi spans both books (900 + 300); classic only b2 (300).
    assert_eq!(s.top_tags[0].name, "sci-fi");
    assert_eq!(s.top_tags[0].seconds, 1200);
    assert_eq!(s.top_tags[1].name, "classic");
}

#[tokio::test]
async fn empty_library_returns_zeroed_summary() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert!(s.is_empty());
    assert_eq!(s.total_seconds(), 0);
    assert_eq!(s.sessions, 0);
    assert_eq!(s.active_days, 0);
    assert_eq!(s.longest_streak_days, 0);
    assert_eq!(s.books_finished, 0);
    assert!(s.heatmap.is_empty());
    assert!(s.top_authors.is_empty());
    assert!(s.busiest_week_start.is_none());
}

#[tokio::test]
async fn stats_are_scoped_to_the_requesting_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;

    reading_session(&pool, alice, "uuid-1", T0, 600).await;

    let bob_stats = compute(&pool, bob, StatsRange::AllTime, 0).await.unwrap();
    assert!(bob_stats.is_empty());
}

#[tokio::test]
async fn cache_serves_within_ttl_and_refreshes_after_expiry() {
    // The cache is a process-wide static; clear it so this test is
    // order-independent regardless of what other tests primed.
    clear_cache();
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "cache-user").await;

    reading_session(&pool, user, "uuid-1", T0, 600).await;

    // Prime the cache at t=1000.
    let first = user_stats_at(&pool, user, StatsRange::AllTime, 0, 1000)
        .await
        .unwrap();
    assert_eq!(first.reading_seconds, 600);

    // A new session lands, but a call inside the TTL still sees the cached value.
    reading_session(&pool, user, "uuid-1", T0 + DAY, 900).await;
    let cached = user_stats_at(
        &pool,
        user,
        StatsRange::AllTime,
        0,
        1000 + STATS_TTL_SECS - 1,
    )
    .await
    .unwrap();
    assert_eq!(cached.reading_seconds, 600);

    // Past the TTL the SQL re-runs and picks up the new session.
    let refreshed = user_stats_at(&pool, user, StatsRange::AllTime, 0, 1000 + STATS_TTL_SECS)
        .await
        .unwrap();
    assert_eq!(refreshed.reading_seconds, 1500);
}

#[tokio::test]
async fn listening_daily_sums_seconds_per_day_within_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    listening_session(&pool, user, "uuid-1", T0, 300).await;
    listening_session(&pool, user, "uuid-1", T0 + 100, 200).await;
    listening_session(&pool, user, "uuid-1", T0 + DAY, 400).await;

    let daily = listening_daily(&pool, user, T0, 0).await.unwrap();
    assert_eq!(daily.len(), 2);
    assert_eq!(daily[0].seconds, 500);
    assert_eq!(daily[1].seconds, 400);
}

/// Seed a user with an explicit id. The stats cache is a process-wide static
/// keyed on `(user_id, range)` and every test pool restarts ids at 1, so
/// tests exercising the *cached* `user_stats` entry point must claim ids no
/// sibling test can collide with (tests run in parallel — a `clear_cache()`
/// here would race the TTL test instead of helping).
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
async fn user_stats_returns_summary_through_the_public_entry_point() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user_with_id(&pool, 9901, "entry-point").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let summary = user_stats(&pool, user, StatsRange::AllTime, None)
        .await
        .unwrap();

    assert_eq!(summary.reading_seconds, 600);
    assert_eq!(summary.sessions, 1);
}

#[tokio::test]
async fn user_stats_surfaces_sqlx_error_when_sessions_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user_with_id(&pool, 9902, "broken-db").await;
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let err = user_stats(&pool, user, StatsRange::AllTime, None)
        .await
        .unwrap_err();
    assert!(matches!(err, StatsError::Sqlx(_)), "got: {err:?}");
}
