//! Unit tests for `db::stats`: per-metric happy paths over seeded sessions,
//! the empty-library case, and the 60s TTL cache contract (fresh-within-window
//! vs refresh-after-expiry).

use omnibus_shared::{PeriodComparison, StatsRange};

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

async fn rate_book(pool: &SqlitePool, user: i64, uuid: &str, half_stars: i64, updated_at: i64) {
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(half_stars)
    .bind(updated_at)
    .execute(pool)
    .await
    .unwrap();
}

/// Unix seconds `months` calendar-months before the DB's `now` — computed by
/// SQLite itself so it lands in the same calendar month the trailing-12
/// recursive CTE anchors on, regardless of day-of-month clamping.
async fn months_ago_secs(pool: &SqlitePool, months: i64) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT CAST(strftime('%s', 'now', '-{months} months') AS INTEGER)"
    ))
    .fetch_one(pool)
    .await
    .unwrap()
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
    // T0 (2023) predates the trailing-12-month window, so this fixture
    // finish doesn't land in it — just confirm the shape is always 12.
    assert_eq!(s.books_per_month.len(), 12);
}

/// Stamp an explicit read-status `finished` on a book at `finished_at`.
async fn finish_read_status(pool: &SqlitePool, user: i64, uuid: &str, finished_at: i64) {
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

#[tokio::test]
async fn finished_books_count_explicit_read_status_finishes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;

    // book 1: finished via read-status only; book 2: journal only; book 3:
    // read-status 'reading' (must not count).
    finish_read_status(&pool, user, "uuid-1", T0).await;
    finish_journal(&pool, user, "uuid-2", T0).await;
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at)
         VALUES (?, 'uuid-3', 'reading', ?)",
    )
    .bind(user)
    .bind(T0)
    .execute(&pool)
    .await
    .unwrap();

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.books_finished, 2);
    let uuids: Vec<&str> = s
        .finished_books
        .iter()
        .map(|b| b.book_uuid.as_str())
        .collect();
    assert!(uuids.contains(&"uuid-1"));
    assert!(uuids.contains(&"uuid-2"));
    assert!(!uuids.contains(&"uuid-3"));
}

#[tokio::test]
async fn book_finished_both_ways_counts_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    finish_journal(&pool, user, "uuid-1", T0).await;
    finish_read_status(&pool, user, "uuid-1", T0 + 100).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.books_finished, 1);
    assert_eq!(s.finished_books.len(), 1);
    // The rail's finish time is the newest completion moment across sources.
    assert_eq!(s.finished_books[0].finished_at, T0 + 100);
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
async fn genre_share_counts_distinct_books_per_tag_not_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;
    let b1 = book_id(&pool, "uuid-1").await;
    let b2 = book_id(&pool, "uuid-2").await;
    let b3 = book_id(&pool, "uuid-3").await;

    // sci-fi spans two active books; classic one (but with FAR more seconds —
    // count-ranking must ignore that); horror tags only an inactive book.
    link_tag(&pool, b1, "sci-fi").await;
    link_tag(&pool, b2, "sci-fi").await;
    link_tag(&pool, b1, "classic").await;
    link_tag(&pool, b3, "horror").await;

    reading_session(&pool, user, "uuid-1", T0, 90_000).await;
    reading_session(&pool, user, "uuid-1", T0 + DAY, 90_000).await;
    listening_session(&pool, user, "uuid-2", T0, 60).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

    assert_eq!(s.genre_share.len(), 2, "inactive book's tag is excluded");
    assert_eq!(s.genre_share[0].name, "sci-fi");
    assert_eq!(s.genre_share[0].books, 2);
    assert_eq!(s.genre_share[1].name, "classic");
    assert_eq!(s.genre_share[1].books, 1);
    // Two distinct active books, sessions on both tables.
    assert_eq!(s.books_active, 2);
    // Stamped from the server clock: a real YYYY-MM-DD.
    assert_eq!(s.as_of_day.len(), 10);
}

#[tokio::test]
async fn genre_share_is_scoped_to_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let b1 = book_id(&pool, "uuid-1").await;
    link_tag(&pool, b1, "sci-fi").await;

    // Only pre-window activity → no genre share inside the window.
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let share = genre_share(&pool, user, T0 + DAY).await.unwrap();
    assert!(share.is_empty());
    assert_eq!(books_active(&pool, user, T0 + DAY).await.unwrap(), 0);
}

#[tokio::test]
async fn avg_stars_means_ratings_updated_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;

    // 8 half-stars (4.0★) and 9 half-stars (4.5★) → mean 4.25★; a rating
    // updated before the window start must not drag the mean down.
    rate_book(&pool, user, "uuid-1", 8, T0).await;
    rate_book(&pool, user, "uuid-2", 9, T0).await;
    rate_book(&pool, user, "uuid-3", 1, T0 - DAY).await;

    let in_window = avg_stars(&pool, user, T0).await.unwrap();
    assert_eq!(in_window, Some(4.25));

    let all = avg_stars(&pool, user, 0).await.unwrap();
    assert_eq!(all, Some(3.0));
}

#[tokio::test]
async fn avg_stars_is_none_when_nothing_was_rated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    assert_eq!(avg_stars(&pool, user, 0).await.unwrap(), None);

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.avg_stars, None);
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

#[tokio::test]
async fn books_per_month_returns_twelve_months_with_zeroed_gaps_and_excludes_older_finishes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;

    let now = months_ago_secs(&pool, 0).await;
    let five_back = months_ago_secs(&pool, 5).await;
    let thirteen_back = months_ago_secs(&pool, 13).await;
    finish_journal(&pool, user, "uuid-1", now).await;
    finish_journal(&pool, user, "uuid-2", five_back).await;
    // Outside the trailing-12 window — must not appear or widen it.
    finish_journal(&pool, user, "uuid-3", thirteen_back).await;

    let months = books_per_month(&pool, user).await.unwrap();

    assert_eq!(months.len(), 12);
    assert_eq!(months.iter().map(|m| m.books).sum::<i64>(), 2);
    assert_eq!(
        months.last().unwrap().books,
        1,
        "current month has 1 finish"
    );
    assert!(
        months.iter().any(|m| m.books == 0),
        "months without a finish still appear, zeroed: {months:?}"
    );
    let mut sorted = months.clone();
    sorted.sort_by(|a, b| a.month.cmp(&b.month));
    assert_eq!(months, sorted, "months come back oldest-first");
}

#[tokio::test]
async fn books_per_month_never_includes_a_future_month() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    let months = books_per_month(&pool, user).await.unwrap();

    let current: String = sqlx::query_scalar("SELECT strftime('%Y-%m', 'now')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(months.len(), 12);
    assert_eq!(
        months.last().unwrap().month,
        current,
        "trailing window ends at the current month"
    );
    assert!(
        months.iter().all(|m| m.month <= current),
        "no bucket is ahead of the current month: {months:?}"
    );
}

#[tokio::test]
async fn books_per_month_counts_only_hundred_percent_journal_entries() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let now = months_ago_secs(&pool, 0).await;

    finish_journal(&pool, user, "uuid-1", now).await;
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at)
         VALUES (?, 'uuid-2', 'partway', 40, ?)",
    )
    .bind(user)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let months = books_per_month(&pool, user).await.unwrap();
    assert_eq!(months.last().unwrap().books, 1);
}

#[tokio::test]
async fn books_per_month_is_empty_of_finishes_for_a_user_with_no_activity() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    let months = books_per_month(&pool, user).await.unwrap();
    assert_eq!(months.len(), 12);
    assert_eq!(months.iter().map(|m| m.books).sum::<i64>(), 0);
}

#[tokio::test]
async fn finished_books_carry_cover_url_only_when_the_book_has_a_cover() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE uuid = 'uuid-1'")
        .execute(&pool)
        .await
        .unwrap();

    finish_journal(&pool, user, "uuid-1", T0).await;
    finish_journal(&pool, user, "uuid-2", T0).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    let by_uuid = |u: &str| s.finished_books.iter().find(|b| b.book_uuid == u).unwrap();
    assert_eq!(
        by_uuid("uuid-1").cover_url.as_deref(),
        Some("/api/covers/uuid-1")
    );
    assert_eq!(by_uuid("uuid-2").cover_url, None);
}

#[tokio::test]
async fn finished_books_carry_the_users_rating_when_rated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    rate_book(&pool, user, "uuid-1", 9, T0).await;
    finish_journal(&pool, user, "uuid-1", T0).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.finished_books[0].rating, Some(4.5));
}

#[tokio::test]
async fn finished_books_rating_is_none_when_unrated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    finish_journal(&pool, user, "uuid-1", T0).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();
    assert_eq!(s.finished_books[0].rating, None);
}

#[tokio::test]
async fn previous_period_is_zeroed_for_all_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    listening_session(&pool, user, "uuid-1", T0, 600).await;

    let prev = previous_period(&pool, user, StatsRange::AllTime)
        .await
        .unwrap();
    assert_eq!(prev, PeriodComparison::default());
}

#[tokio::test]
async fn previous_period_month_sums_only_last_calendar_months_activity() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let last_month = months_ago_secs(&pool, 1).await;
    let two_months_back = months_ago_secs(&pool, 2).await;
    let now = now_secs();

    listening_session(&pool, user, "uuid-1", last_month, 500).await;
    // Outside the previous window — must not be counted.
    listening_session(&pool, user, "uuid-1", two_months_back, 999).await;
    listening_session(&pool, user, "uuid-1", now, 111).await;
    rate_book(&pool, user, "uuid-1", 10, last_month).await;
    finish_journal(&pool, user, "uuid-1", last_month).await;

    let prev = previous_period(&pool, user, StatsRange::Month)
        .await
        .unwrap();
    assert_eq!(prev.listening_seconds, 500);
    assert_eq!(prev.avg_stars, Some(5.0));
    assert_eq!(prev.books_finished, 1);
}

#[tokio::test]
async fn listening_daily_sums_seconds_per_day_within_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    listening_session(&pool, user, "uuid-1", T0, 300).await;
    listening_session(&pool, user, "uuid-1", T0 + 100, 200).await;
    listening_session(&pool, user, "uuid-1", T0 + DAY, 400).await;

    let daily = listening_daily(&pool, user, T0).await.unwrap();
    assert_eq!(daily.len(), 2);
    assert_eq!(daily[0].seconds, 500);
    assert_eq!(daily[1].seconds, 400);
}

#[tokio::test]
async fn rating_monthly_returns_twelve_months_zeroed_when_unrated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    let months = rating_monthly(&pool, user).await.unwrap();
    assert_eq!(months.len(), 12);
    assert!(months.iter().all(|m| m.value == 0.0));
}

#[tokio::test]
async fn rating_monthly_places_a_rating_in_its_calendar_month() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let now = months_ago_secs(&pool, 0).await;
    rate_book(&pool, user, "uuid-1", 7, now).await;

    let months = rating_monthly(&pool, user).await.unwrap();
    assert_eq!(months.last().unwrap().value, 3.5);
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

    let summary = user_stats(&pool, user, StatsRange::AllTime).await.unwrap();

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

    let err = user_stats(&pool, user, StatsRange::AllTime)
        .await
        .unwrap_err();
    assert!(matches!(err, StatsError::Sqlx(_)), "got: {err:?}");
}

#[tokio::test]
async fn finished_books_rail_is_capped_but_count_is_not() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, FINISHED_BOOKS_LIMIT + 5).await;
    let user = seed_user(&pool, "finisher").await;
    for i in 1..=(FINISHED_BOOKS_LIMIT + 5) {
        finish_journal(&pool, user, &format!("uuid-{i}"), T0 + i).await;
    }

    let rail = finished_books(&pool, user, 0).await.unwrap();
    let total = finished_count(&pool, user, 0).await.unwrap();

    assert_eq!(rail.len() as i64, FINISHED_BOOKS_LIMIT);
    assert_eq!(total, FINISHED_BOOKS_LIMIT + 5);
    // Newest completions win the capped rail.
    assert_eq!(
        rail[0].book_uuid,
        format!("uuid-{}", FINISHED_BOOKS_LIMIT + 5)
    );
}
