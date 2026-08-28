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

/// Assign genres to a book. Genres have no link table (migration `0066`) —
/// they exist only as a `metadata_overrides` entry — so this goes through
/// the real write path, which also materializes the `genres` rows the
/// donut's join needs.
async fn set_genres(pool: &SqlitePool, uuid: &str, names: &[&str], user: i64) {
    crate::merge_metadata_overrides(
        pool,
        uuid,
        &omnibus_shared::MetadataOverrides {
            genres: Some(names.iter().map(|n| (*n).to_string()).collect()),
            ..Default::default()
        },
        user,
    )
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
        // Mid-month anchor: naive '-N months' from a month-end 'now'
        // (July 31 → "June 31" → July 1) lands the seed in the wrong month.
        "SELECT CAST(strftime('%s', 'now', 'start of month', '-{months} months', '+14 days') AS INTEGER)"
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
async fn session_count_stitches_contiguous_checkpoint_rows_into_one_sitting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // An hour of web reading as sixty 60s heartbeat flushes.
    for i in 0..60 {
        reading_session(&pool, user, "uuid-1", T0 + i * 60, 60).await;
    }

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

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

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

    assert_eq!(s.sessions, 2);
}

#[tokio::test]
async fn session_count_excludes_glances_but_keeps_their_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + DAY, 20).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

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
async fn genre_share_counts_distinct_books_per_genre_not_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;

    // Sci-Fi spans two active books; Classic one (but with FAR more seconds —
    // count-ranking must ignore that); Horror is on an inactive book only.
    set_genres(&pool, "uuid-1", &["Sci-Fi", "Classic"], user).await;
    set_genres(&pool, "uuid-2", &["Sci-Fi"], user).await;
    set_genres(&pool, "uuid-3", &["Horror"], user).await;

    reading_session(&pool, user, "uuid-1", T0, 90_000).await;
    reading_session(&pool, user, "uuid-1", T0 + DAY, 90_000).await;
    listening_session(&pool, user, "uuid-2", T0, 60).await;

    let s = compute(&pool, user, StatsRange::AllTime).await.unwrap();

    assert_eq!(s.genre_share.len(), 2, "inactive book's genre is excluded");
    assert_eq!(s.genre_share[0].name, "Sci-Fi");
    assert_eq!(s.genre_share[0].books, 2);
    assert_eq!(s.genre_share[1].name, "Classic");
    assert_eq!(s.genre_share[1].books, 1);
    // Two distinct active books, sessions on both tables.
    assert_eq!(s.books_active, 2);
    // Stamped from the server clock: a real YYYY-MM-DD.
    assert_eq!(s.as_of_day.len(), 10);
}

#[tokio::test]
async fn genre_share_ignores_tags_so_the_donut_only_reports_assigned_genres() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    let b1 = book_id(&pool, "uuid-1").await;

    // A heavily-tagged book with no genres contributes nothing: "What you
    // read" reports genres, and a `<dc:subject>` list is not one.
    link_tag(&pool, b1, "sci-fi").await;
    link_tag(&pool, b1, "classic").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    assert!(genre_share(&pool, user, T0 - DAY).await.unwrap().is_empty());

    // Assigning a genre to the same book makes it appear.
    set_genres(&pool, "uuid-1", &["Space Opera"], user).await;
    let share = genre_share(&pool, user, T0 - DAY).await.unwrap();
    assert_eq!(share.len(), 1);
    assert_eq!(share[0].name, "Space Opera");
}

#[tokio::test]
async fn genre_share_folds_case_variants_into_one_slice() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;

    // `genres.name` is NOCASE-unique, so the second spelling deduplicates
    // into the row the first coined — one slice of two books, not two of one.
    set_genres(&pool, "uuid-1", &["Sci-Fi"], user).await;
    set_genres(&pool, "uuid-2", &["sci-fi"], user).await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-2", T0, 600).await;

    let share = genre_share(&pool, user, T0 - DAY).await.unwrap();
    assert_eq!(share.len(), 1, "case variants fold together");
    assert_eq!(share[0].name, "Sci-Fi", "first spelling coined the row");
    assert_eq!(share[0].books, 2);
}

#[tokio::test]
async fn genre_share_is_scoped_to_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    set_genres(&pool, "uuid-1", &["Sci-Fi"], user).await;

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
    // Anchored to the first second of last month rather than its middle: the
    // baseline is the *elapsed* slice of the previous period, so a mid-month
    // seed would fall outside it whenever the suite runs early in a month.
    let last_month = prev_period_start(&pool, StatsRange::Month).await;
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

/// Drop a book row the way `merge::transaction::finalize_merge` used to,
/// leaving its soft-referencing user-data rows behind. Migration `0079` heals
/// the rows an old merge stranded and the merge path no longer strands new
/// ones — but a completion can outlive its book by any route that drops a
/// `books` row, and every metric must agree about that book either way.
async fn drop_book_row(pool: &SqlitePool, uuid: &str) {
    sqlx::query("DELETE FROM books WHERE uuid = ?")
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

async fn now_secs_db(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT CAST(strftime('%s','now') AS INTEGER)")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn finished_metrics_agree_when_a_completion_outlives_its_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let now = now_secs_db(&pool).await;
    finish_read_status(&pool, user, "uuid-1", now - 60).await;
    finish_read_status(&pool, user, "uuid-2", now - 60).await;
    drop_book_row(&pool, "uuid-2").await;

    // The headline count, the rail and the trailing-12 chart are three reads
    // of one definition; before the shared liveness filter the chart reported
    // 2 while the tile above it reported 1, for the same month.
    let headline = finished_count(&pool, user, 0).await.unwrap();
    let rail = finished_books(&pool, user, 0).await.unwrap();
    let months = books_per_month(&pool, user).await.unwrap();
    assert_eq!(headline, 1);
    assert_eq!(rail.len(), 1);
    assert_eq!(months.last().unwrap().books, 1);
}

#[tokio::test]
async fn previous_period_excludes_a_completion_whose_book_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    // The first second of last month. Not mid-month: the baseline is now the
    // *elapsed* slice of the previous period, so a day-15 seed falls outside it
    // for the first fortnight of every month.
    let prev = prev_period_start(&pool, StatsRange::Month).await;
    finish_read_status(&pool, user, "uuid-1", prev).await;
    finish_read_status(&pool, user, "uuid-2", prev).await;
    drop_book_row(&pool, "uuid-2").await;

    // The delta's baseline must count the same population the current window
    // does, or the drill-in invents a drop the reader never had.
    let previous = previous_period(&pool, user, StatsRange::Month)
        .await
        .unwrap();
    assert_eq!(previous.books_finished, 1);
}

#[tokio::test]
async fn avg_stars_excludes_a_rating_whose_book_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let now = now_secs_db(&pool).await;
    rate_book(&pool, user, "uuid-1", 10, now - 60).await;
    rate_book(&pool, user, "uuid-2", 2, now - 60).await;
    drop_book_row(&pool, "uuid-2").await;

    // The 1-star sits on a book the UI cannot render a rating for, so it must
    // not drag the mean the stats page shows.
    assert_eq!(avg_stars(&pool, user, 0).await.unwrap(), Some(5.0));
}

#[tokio::test]
async fn rating_monthly_excludes_a_rating_whose_book_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let now = now_secs_db(&pool).await;
    rate_book(&pool, user, "uuid-1", 10, now - 60).await;
    rate_book(&pool, user, "uuid-2", 2, now - 60).await;
    drop_book_row(&pool, "uuid-2").await;

    let months = rating_monthly(&pool, user).await.unwrap();
    assert_eq!(months.len(), 12);
    // Same filter as `avg_stars`, so the tile and its trend agree.
    assert!(
        (months.last().unwrap().value - 5.0).abs() < f64::EPSILON,
        "expected the current month to mean 5.0, got {months:?}"
    );
}

/// First second of the period preceding `range`'s current one.
async fn prev_period_start(pool: &SqlitePool, range: StatsRange) -> i64 {
    prev_window_bounds(pool, range).await.unwrap().unwrap().0
}

#[tokio::test]
async fn prev_window_bounds_cover_the_elapsed_slice_of_the_previous_period() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    for range in [StatsRange::Week, StatsRange::Month, StatsRange::Year] {
        let (start, end) = prev_window_bounds(&pool, range).await.unwrap().unwrap();
        let cur_start = window_start(&pool, range).await.unwrap();
        let elapsed = now_secs() - cur_start;

        // The baseline sits wholly before the current window …
        assert!(
            end <= cur_start,
            "{range:?}: baseline must not overlap the current window ({end} > {cur_start})"
        );
        // Empty only at the exact first second of a period, where the current
        // window is equally empty — asserted as a bound, not as non-emptiness.
        assert!(start <= end, "{range:?}: baseline must not be inverted");
        // … and covers the same elapsed offset, never the whole period. The
        // slack absorbs the second or two between the two `now` reads; the
        // clamp can only ever make the slice shorter, never longer.
        let slice = end - start;
        assert!(
            slice <= elapsed + 2,
            "{range:?}: baseline slice {slice}s exceeds the elapsed {elapsed}s"
        );
    }
}

#[tokio::test]
async fn previous_period_aggregates_only_within_the_baseline_bounds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Seeded from the bounds themselves, so this asserts that the aggregates
    // honour the window — that the window is the *right* window is
    // `prev_window_bounds_cover_the_elapsed_slice_of_the_previous_period`'s
    // job, and only that test detects a regression in the bounds arithmetic.
    let (start, end) = prev_window_bounds(&pool, StatsRange::Week)
        .await
        .unwrap()
        .unwrap();
    listening_session(&pool, user, "uuid-1", start, 500).await;
    // `end` is exclusive. Seeded a second past it: `previous_period` re-reads
    // the bounds, and for Week `end` advances with the wall clock, so a seed
    // exactly on it can slip inside when a second boundary falls between.
    listening_session(&pool, user, "uuid-1", end + 1, 999).await;
    listening_session(&pool, user, "uuid-1", start - 1, 777).await;

    let prev = previous_period(&pool, user, StatsRange::Week)
        .await
        .unwrap();
    assert_eq!(prev.listening_seconds, 500);
}

#[test]
fn prev_window_from_takes_the_elapsed_offset_into_the_previous_period() {
    // Fixed dates, so the clamp and the degenerate cases are exercised on every
    // run rather than on the handful of calendar days that reach them live.
    const DAY: i64 = 86_400;
    // 2026-03-01 00:00 UTC, 2026-02-01 00:00 UTC.
    let mar1 = 1_772_323_200;
    let feb1 = mar1 - 28 * DAY;

    // Three days into March → the first three days of February.
    let (s, e) = prev_window_from(feb1, mar1, mar1 + 3 * DAY);
    assert_eq!((s, e), (feb1, feb1 + 3 * DAY));

    // Thirty days into March → clamped to the whole of a 28-day February,
    // never past the period it belongs to.
    let (s, e) = prev_window_from(feb1, mar1, mar1 + 30 * DAY);
    assert_eq!((s, e), (feb1, mar1));
    assert_eq!(e - s, 28 * DAY);

    // The exact first second of the period: nothing elapsed, so the baseline
    // is empty rather than the whole previous month.
    assert_eq!(prev_window_from(feb1, mar1, mar1), (feb1, feb1));

    // A clock that reads behind the period start cannot invert the window.
    assert_eq!(prev_window_from(feb1, mar1, mar1 - 5), (feb1, feb1));
}
