//! Unit tests for `db::stats`: per-metric happy paths over seeded sessions,
//! the empty-library case, and the 60s TTL cache contract (fresh-within-window
//! vs refresh-after-expiry).

use omnibus_shared::StatsRange;

use super::*;
use crate::init_db;
use crate::test_support::seed_minimal_books;

const DAY: i64 = 86_400;

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

/// The `books.id` behind a seeded `uuid-N`, for the taxonomy link helpers.
async fn book_id(pool: &SqlitePool, uuid: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn reading_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO reading_sessions (user_id, book_uuid, started_at, ended_at, seconds_read)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

async fn listening_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO listening_sessions (user_id, book_uuid, started_at, ended_at, seconds_listened)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

async fn link_author(pool: &SqlitePool, book: i64, name: &str) {
    sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let author: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book)
        .bind(author)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_tag(pool: &SqlitePool, book: i64, name: &str) {
    sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let tag: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(book)
        .bind(tag)
        .execute(pool)
        .await
        .unwrap();
}

async fn finish_journal(pool: &SqlitePool, user: i64, uuid: &str, created_at: i64) {
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at)
         VALUES (?, ?, 'done', 100, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap();
}

/// A late-2023 anchor: an all-time window covers it, "this year" (2026+) does not.
const T0: i64 = 1_700_000_000;

#[tokio::test]
async fn all_time_aggregates_hours_sessions_and_active_days() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;

    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + DAY, 1200).await;
    listening_session(&pool, user, "uuid-1", T0 + 2 * DAY, 300).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

    assert_eq!(s.reading_seconds, 1800);
    assert_eq!(s.listening_seconds, 300);
    assert_eq!(s.total_seconds(), 2100);
    assert_eq!(s.sessions, 3);
    assert_eq!(s.active_days, 3);
    assert_eq!(s.heatmap.len(), 3);
    assert_eq!(s.heatmap.iter().map(|d| d.seconds).sum::<i64>(), 2100);
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

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.active_days, 5);
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

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.busiest_week_seconds, 5000);
    assert!(s.busiest_week_start.is_some());
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

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

    assert_eq!(s.top_authors[0].name, "Ursula K. Le Guin");
    assert_eq!(s.top_authors[0].seconds, 900);
    assert_eq!(s.top_authors[1].name, "Isaac Asimov");
    // sci-fi spans both books (900 + 300); classic only b2 (300).
    assert_eq!(s.top_tags[0].name, "sci-fi");
    assert_eq!(s.top_tags[0].seconds, 1200);
    assert_eq!(s.top_tags[1].name, "classic");
}

#[tokio::test]
async fn finished_books_come_from_hundred_percent_journal_entries() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let b1 = book_id(&pool, "uuid-1").await;
    link_author(&pool, b1, "Ursula K. Le Guin").await;

    // A 100% entry finishes book 1; a partial entry on book 2 does not count.
    finish_journal(&pool, user, "uuid-1", T0).await;
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at)
         VALUES (?, 'uuid-2', 'partway', 40, ?)",
    )
    .bind(user)
    .bind(T0)
    .execute(&pool)
    .await
    .unwrap();

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.books_finished, 1);
    assert_eq!(s.finished_books.len(), 1);
    assert_eq!(s.finished_books[0].book_uuid, "uuid-1");
    assert_eq!(
        s.finished_books[0].author.as_deref(),
        Some("Ursula K. Le Guin")
    );
}

#[tokio::test]
async fn empty_library_returns_zeroed_summary() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
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

    let bob_stats = compute(&pool, bob, StatsRange::AllTime).await.unwrap();
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
    let first = user_stats_at(&pool, user, StatsRange::AllTime, 1000)
        .await
        .unwrap();
    assert_eq!(first.reading_seconds, 600);

    // A new session lands, but a call inside the TTL still sees the cached value.
    reading_session(&pool, user, "uuid-1", T0 + DAY, 900).await;
    let cached = user_stats_at(&pool, user, StatsRange::AllTime, 1000 + STATS_TTL_SECS - 1)
        .await
        .unwrap();
    assert_eq!(cached.reading_seconds, 600);

    // Past the TTL the SQL re-runs and picks up the new session.
    let refreshed = user_stats_at(&pool, user, StatsRange::AllTime, 1000 + STATS_TTL_SECS)
        .await
        .unwrap();
    assert_eq!(refreshed.reading_seconds, 1500);
}

#[tokio::test]
async fn week_window_keeps_only_the_rolling_last_seven_days() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // 8 days back is outside the rolling window even at start-of-day
    // granularity; a just-now session is inside it.
    let now = now_secs();
    reading_session(&pool, user, "uuid-1", now - 8 * DAY, 600).await;
    reading_session(&pool, user, "uuid-1", now, 300).await;

    let s = compute(&pool, user, StatsRange::Week).await.unwrap();
    assert_eq!(s.reading_seconds, 300);
    assert_eq!(s.sessions, 1);
}

#[tokio::test]
async fn month_window_starts_at_the_first_of_the_current_month() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // 32 days back always lands in a previous month; a just-now session is in
    // the current one.
    let now = now_secs();
    reading_session(&pool, user, "uuid-1", now - 32 * DAY, 600).await;
    reading_session(&pool, user, "uuid-1", now, 300).await;

    let s = compute(&pool, user, StatsRange::Month).await.unwrap();
    assert_eq!(s.reading_seconds, 300);
    assert_eq!(s.sessions, 1);
}

#[tokio::test]
async fn year_window_excludes_old_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // T0 is in 2023; the Year window (current calendar year) excludes it.
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let s = compute(&pool, user, StatsRange::Year).await.unwrap();
    assert_eq!(s.reading_seconds, 0);
    assert!(s.is_empty());
}
