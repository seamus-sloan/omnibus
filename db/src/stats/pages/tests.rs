//! Unit tests for the length ladder and the aggregates over it —
//! [`super::pages_read`] and its detail, [`super::pages_per_hour`], and
//! [`super::length_buckets`]. Every input is a persisted column, so these seed
//! those columns directly: no EPUB or CBZ is opened at query time, and the
//! forward-progress ledger is seeded rather than driven through the position
//! write path (covered in `db::progress::ledger`).

use super::*;
use crate::init_db;
use crate::test_support::seed_user;

const T0: i64 = 1_700_000_000;

async fn seed_lib(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed one `books` row with explicit `word_count` / `page_count` (NULL when
/// `None`) — the two lower rungs of the ladder.
async fn seed_book(
    pool: &SqlitePool,
    lib_id: i64,
    uuid: &str,
    word_count: Option<i64>,
    page_count: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, word_count, page_count)
         VALUES (?, ?, '', ?, ?, ?)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .bind(word_count)
    .bind(page_count)
    .execute(pool)
    .await
    .unwrap();
}

/// The ladder's top rung: a print-edition page count in the override blob.
async fn set_print_pages(pool: &SqlitePool, uuid: &str, print_pages: i64) {
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides)
         VALUES (?, json_object('print_pages', ?))",
    )
    .bind(uuid)
    .bind(print_pages)
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

async fn finish_read_status(pool: &SqlitePool, user: i64, uuid: &str, finished_at: i64) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, finished_at)
         VALUES (?, ?, 'finished', ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(finished_at)
    .execute(pool)
    .await
    .unwrap();
}

async fn read_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
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

async fn listen_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened)
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

/// Seed one day's forward progress on a book — the ledger rows
/// `db::progress::ledger` writes from the position path.
async fn read_percent(pool: &SqlitePool, user: i64, uuid: &str, day: &str, gained: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily
             (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(gained)
    .execute(pool)
    .await
    .unwrap();
}

/// The UTC day `T0` falls in — every ledger row here is seeded on it unless a
/// test is specifically about day bucketing.
const T0_DAY: &str = "2023-11-14";

/// The count in a named bucket, or -1 when the bucket is missing entirely.
fn books_in(buckets: &[LengthBucket], label: &str) -> i64 {
    buckets
        .iter()
        .find(|b| b.label == label)
        .map(|b| b.books)
        .unwrap_or(-1)
}

// --- the ladder ---------------------------------------------------------

#[tokio::test]
async fn pages_read_prefers_print_pages_over_every_estimate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // All three rungs available: the real print count must win over the CBZ
    // image count and the word estimate alike.
    seed_book(&pool, lib, "uuid-a", Some(275), Some(30)).await;
    set_print_pages(&pool, "uuid-a", 412).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 100).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(412));
}

#[tokio::test]
async fn pages_read_uses_the_comic_page_count_when_no_print_pages_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A CBZ carries an exact image-page count and no word count.
    seed_book(&pool, lib, "uuid-a", None, Some(32)).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 100).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(32));
}

#[tokio::test]
async fn pages_read_prefers_the_comic_page_count_over_the_word_estimate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A comic whose word count was also backfilled. The image count is exact
    // and the word estimate is not, so the ladder's middle rung must win —
    // swapping the two COALESCE arms is otherwise invisible to every other
    // test here, since each covers a rung in isolation.
    seed_book(&pool, lib, "uuid-a", Some(275 * 40), Some(32)).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 100).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(32));
}

#[tokio::test]
async fn pages_read_falls_back_to_the_word_count_estimate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(275), None).await; // 1 page
    seed_book(&pool, lib, "uuid-b", Some(550), None).await; // 2 pages
    read_percent(&pool, user, "uuid-a", T0_DAY, 100).await;
    read_percent(&pool, user, "uuid-b", T0_DAY, 100).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(3));
}

#[tokio::test]
async fn pages_read_rounds_the_word_estimate_to_the_nearest_page() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // 137 words is 0.498 pages, 138 is 0.502 — a 260-word "book" is one page,
    // not zero.
    seed_book(&pool, lib, "uuid-a", Some(137), None).await;
    seed_book(&pool, lib, "uuid-b", Some(138), None).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 100).await;
    read_percent(&pool, user, "uuid-b", T0_DAY, 100).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(1));
}

// --- pages_read ---------------------------------------------------------

#[tokio::test]
async fn pages_read_counts_part_of_a_book_without_any_completion() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // The whole point of the metric: a reader steadily through a quarter of a
    // 400-page book has read 100 pages, and nothing was finished to say so.
    seed_book(&pool, lib, "uuid-a", None, None).await;
    set_print_pages(&pool, "uuid-a", 400).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 25).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(100));
}

#[tokio::test]
async fn pages_read_ignores_a_finish_that_covered_no_ground_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A 600-page book read months ago and only *marked* finished now. The flip
    // must not dump its length into this window.
    seed_book(&pool, lib, "uuid-a", None, Some(600)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_read_status(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_sums_every_book_and_day_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    seed_book(&pool, lib, "uuid-b", None, Some(400)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-14", 10).await; // 20
    read_percent(&pool, user, "uuid-a", "2023-11-15", 15).await; // 30
    read_percent(&pool, user, "uuid-b", "2023-11-15", 25).await; // 100

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(150));
}

#[tokio::test]
async fn pages_read_rounds_the_total_once_rather_than_per_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Three books at 0.4 pages each: rounded per book that is zero, rounded
    // once it is one. A reader who turned a few pages in several books should
    // not lose them all to rounding.
    for uuid in ["uuid-a", "uuid-b", "uuid-c"] {
        seed_book(&pool, lib, uuid, None, Some(40)).await;
        read_percent(&pool, user, uuid, T0_DAY, 1).await;
    }

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(1));
}

#[tokio::test]
async fn pages_read_is_none_when_nothing_was_read_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(550), None).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_is_none_when_no_book_read_resolves_a_length() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Read, but every rung is NULL (not-yet-backfilled) — an unmeasured book
    // contributes nothing rather than zero.
    seed_book(&pool, lib, "uuid-a", None, None).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 40).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_ground_covered_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-14", 50).await;

    // A window starting the next day must not see it.
    assert_eq!(pages_read(&pool, user, T0 + 86_400).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_another_users_reading() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    read_percent(&pool, bob, "uuid-a", T0_DAY, 50).await;

    assert_eq!(pages_read(&pool, alice, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_a_ghosted_book_with_no_live_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // A ledger row against a uuid with no `books` row (ghosted): the join
    // drops it, so there is no data.
    read_percent(&pool, user, "uuid-ghost", T0_DAY, 60).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_propagates_sqlx_error_when_the_ledger_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE reading_progress_daily")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_read(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}

// --- pages_read_bounded -------------------------------------------------

#[tokio::test]
async fn pages_read_bounded_covers_its_slice_inclusive_of_the_boundary_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(100)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-13", 10).await; // before
    read_percent(&pool, user, "uuid-a", "2023-11-14", 20).await; // in
    read_percent(&pool, user, "uuid-a", "2023-11-15", 40).await; // after

    // The ledger is day-grained and the window it is compared against always
    // carries a partial today, so the boundary day is included.
    let pages = pages_read_bounded(&pool, user, T0, T0).await.unwrap();

    assert_eq!(pages, 20);
}

#[tokio::test]
async fn pages_read_bounded_is_zero_rather_than_none_for_an_empty_baseline() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert_eq!(pages_read_bounded(&pool, user, 0, T0).await.unwrap(), 0);
}

// --- pages_detail -------------------------------------------------------

#[tokio::test]
async fn pages_detail_separates_an_audio_only_window_from_an_empty_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-audio", None, None).await;
    listen_session(&pool, user, "uuid-audio", T0, 600).await;

    let detail = pages_detail(&pool, user, 0).await.unwrap();

    assert_eq!(detail.audio_books, 1);
    assert_eq!(detail.measured_books, 0);
    assert_eq!(detail.unmeasured_books, 0);
    // The one empty state whose honest headline is zero, not an em-dash.
    assert!(detail.audio_only());
}

#[tokio::test]
async fn pages_detail_reports_an_empty_window_as_not_audio_only() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let detail = pages_detail(&pool, user, 0).await.unwrap();

    assert_eq!(detail.audio_books, 0);
    assert!(!detail.audio_only());
    assert!(detail.daily.is_empty());
}

#[tokio::test]
async fn pages_detail_counts_measured_and_unmeasured_books_apart() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-known", None, Some(120)).await;
    seed_book(&pool, lib, "uuid-unknown", None, None).await;
    read_percent(&pool, user, "uuid-known", T0_DAY, 50).await;
    read_percent(&pool, user, "uuid-unknown", T0_DAY, 50).await;

    let detail = pages_detail(&pool, user, 0).await.unwrap();

    assert_eq!(detail.measured_books, 1);
    // Real reading the total cannot include — a tile that never says so
    // understates itself without admitting it.
    assert_eq!(detail.unmeasured_books, 1);
    assert!(!detail.audio_only());
}

#[tokio::test]
async fn pages_detail_charts_pages_per_day_ascending() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-15", 10).await;
    read_percent(&pool, user, "uuid-a", "2023-11-14", 25).await;

    let detail = pages_detail(&pool, user, 0).await.unwrap();

    let points: Vec<(String, f64)> = detail
        .daily
        .iter()
        .map(|p| (p.label.clone(), p.value))
        .collect();
    assert_eq!(
        points,
        vec![
            ("2023-11-14".to_string(), 50.0),
            ("2023-11-15".to_string(), 20.0),
        ]
    );
}

#[tokio::test]
async fn pages_detail_carries_the_ledger_epoch_so_the_cutover_can_be_stated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let detail = pages_detail(&pool, user, 0).await.unwrap();

    // Reading before this day left no position trail to difference; the
    // surfaces state the date rather than letting the tile change meaning
    // without saying so.
    let since = detail.since_day.expect("migration 0083 records the epoch");
    assert_eq!(since.len(), 10, "expected YYYY-MM-DD, got {since}");
}

#[tokio::test]
async fn pages_detail_propagates_sqlx_error_when_the_ledger_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE reading_progress_daily")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_detail(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}

// --- pages_per_hour -----------------------------------------------------

#[tokio::test]
async fn pages_per_hour_divides_resolved_pages_by_recorded_reading_hours() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    // 300 pages over 10 hours.
    read_session(&pool, user, "uuid-a", T0 - 3600, 10 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_weights_by_seconds_rather_than_averaging_per_book_rates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // 100 pages in 1h (100/h) and 400 pages in 9h (~44/h). A mean of the two
    // per-book rates would be ~72; the seconds-weighted answer is 50, and the
    // long book is the one that describes how this reader actually reads.
    seed_book(&pool, lib, "uuid-fast", None, Some(100)).await;
    seed_book(&pool, lib, "uuid-slow", None, Some(400)).await;
    finish_journal(&pool, user, "uuid-fast", T0).await;
    finish_journal(&pool, user, "uuid-slow", T0).await;
    read_session(&pool, user, "uuid-fast", T0, 3600).await;
    read_session(&pool, user, "uuid-slow", T0, 9 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 50.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_counts_reading_time_from_before_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    // Nine of the ten hours were read long before the window opened. Counting
    // only the in-window hour would report 300 pages/hour for a book that
    // actually took ten.
    read_session(&pool, user, "uuid-a", T0 - 90 * 86_400, 9 * 3600).await;
    read_session(&pool, user, "uuid-a", T0, 3600).await;

    let rate = pages_per_hour(&pool, user, T0 - 3600)
        .await
        .unwrap()
        .unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_excludes_a_finished_book_with_no_recorded_reading_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-timed", None, Some(300)).await;
    // Finished on another device, or before session tracking: its pages with
    // nobody's hours behind them would double the rate.
    seed_book(&pool, lib, "uuid-untimed", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-timed", T0).await;
    finish_journal(&pool, user, "uuid-untimed", T0).await;
    read_session(&pool, user, "uuid-timed", T0, 10 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_excludes_listening_time_from_the_denominator() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;
    // Narration speed is the narrator's; folding it in would drag the rate to
    // 15/h and stop measuring reading at all.
    listen_session(&pool, user, "uuid-a", T0, 10 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_counts_a_book_finished_both_ways_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_read_status(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;

    // Both sides double under a non-DISTINCT scope, so a wrong join here
    // hides in the ratio — the count each side is over is what this pins.
    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_is_none_when_no_finished_book_has_both_a_length_and_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Length but no time.
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    // Time but no resolvable length (audio-only / not-yet-backfilled).
    seed_book(&pool, lib, "uuid-b", None, None).await;
    finish_journal(&pool, user, "uuid-b", T0).await;
    read_session(&pool, user, "uuid-b", T0, 3600).await;

    assert_eq!(pages_per_hour(&pool, user, 0).await.unwrap(), None);
}

/// A `word_count` of 0 is a real stored value — `estimate_word_count` returns
/// `Some(0)` for an EPUB whose spine loads but strips to no words — and the
/// ladder turns it into 0 pages, not NULL. Summing that costs `pages_read`
/// nothing, but here it would donate hours against no pages.
#[tokio::test]
async fn pages_per_hour_excludes_a_zero_page_book_from_both_sides() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;
    // Image-only EPUB: spine loaded, no extractable text.
    seed_book(&pool, lib, "uuid-zero", Some(0), None).await;
    finish_journal(&pool, user, "uuid-zero", T0).await;
    read_session(&pool, user, "uuid-zero", T0, 6 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    // 30/h from the measurable book alone. Counting the zero-page book's
    // six hours in the denominator would give 300/16h = 18.75.
    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_is_none_when_the_only_finished_book_measures_zero_pages() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-zero", Some(0), None).await;
    finish_journal(&pool, user, "uuid-zero", T0).await;
    read_session(&pool, user, "uuid-zero", T0, 6 * 3600).await;

    // Not `Some(0.0)` — "0 pages an hour" is a claim about how this reader
    // reads, and an unmeasurable book is not that claim.
    assert_eq!(pages_per_hour(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_per_hour_is_none_when_nothing_finished_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;

    assert_eq!(pages_per_hour(&pool, user, T0 + 1).await.unwrap(), None);
}

#[tokio::test]
async fn pages_per_hour_ignores_another_users_reading_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let other = seed_user(&pool, "bob").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;
    read_session(&pool, other, "uuid-a", T0, 90 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_propagates_sqlx_error_when_the_books_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_per_hour(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}

#[test]
fn hourly_rate_is_none_without_both_sides_or_with_no_time() {
    assert_eq!(hourly_rate(None, Some(3600)), None);
    assert_eq!(hourly_rate(Some(300), None), None);
    assert_eq!(hourly_rate(Some(300), Some(0)), None);
    assert_eq!(hourly_rate(Some(300), Some(3600)), Some(300.0));
}

// --- length_buckets -----------------------------------------------------

#[tokio::test]
async fn length_buckets_sort_finished_books_by_their_resolved_length() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // One per rung, each landing in a different bucket: a 120-page comic
    // (short), a 412-page print edition (middle), a 700-page word estimate.
    seed_book(&pool, lib, "uuid-comic", None, Some(120)).await;
    seed_book(&pool, lib, "uuid-print", Some(275), None).await;
    set_print_pages(&pool, "uuid-print", 412).await;
    seed_book(&pool, lib, "uuid-epub", Some(700 * 275), None).await;
    for uuid in ["uuid-comic", "uuid-print", "uuid-epub"] {
        finish_journal(&pool, user, uuid, T0).await;
    }

    let buckets = length_buckets(&pool, user, 0).await.unwrap();

    assert_eq!(books_in(&buckets, "Under 300"), 1);
    assert_eq!(books_in(&buckets, "300\u{2013}499"), 1);
    assert_eq!(books_in(&buckets, "500+"), 1);
    assert_eq!(books_in(&buckets, "Unknown"), 0);
}

#[tokio::test]
async fn length_buckets_report_an_unmeasurable_book_as_unknown_not_as_short() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // An audiobook has no page analogue at all; bucketing it as "Under 300"
    // would be a lie about the shape of the window.
    seed_book(&pool, lib, "uuid-audio", None, None).await;
    finish_journal(&pool, user, "uuid-audio", T0).await;

    let buckets = length_buckets(&pool, user, 0).await.unwrap();

    assert_eq!(books_in(&buckets, "Unknown"), 1);
    assert_eq!(books_in(&buckets, "Under 300"), 0);
}

#[tokio::test]
async fn length_buckets_place_the_boundary_pages_in_the_upper_bucket() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Bounds are exclusive upper edges: 299 is short, 300 is middle, 499 is
    // middle, 500 is long.
    for (uuid, pages) in [
        ("uuid-299", 299),
        ("uuid-300", 300),
        ("uuid-499", 499),
        ("uuid-500", 500),
    ] {
        seed_book(&pool, lib, uuid, None, Some(pages)).await;
        finish_journal(&pool, user, uuid, T0).await;
    }

    let buckets = length_buckets(&pool, user, 0).await.unwrap();

    assert_eq!(books_in(&buckets, "Under 300"), 1);
    assert_eq!(books_in(&buckets, "300\u{2013}499"), 2);
    assert_eq!(books_in(&buckets, "500+"), 1);
}

#[tokio::test]
async fn length_buckets_return_every_bucket_zero_filled_for_an_empty_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let buckets = length_buckets(&pool, user, 0).await.unwrap();

    // The spine always comes back; the surfaces read "nothing finished" off
    // the total, not off a missing vec.
    assert_eq!(buckets.len(), LENGTH_BUCKETS.len() + 1);
    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 0);
    assert_eq!(buckets.last().unwrap().label, UNKNOWN_LABEL);
}

#[tokio::test]
async fn length_buckets_count_a_book_finished_both_ways_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(100)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_read_status(&pool, user, "uuid-a", T0).await;

    let buckets = length_buckets(&pool, user, 0).await.unwrap();

    assert_eq!(books_in(&buckets, "Under 300"), 1);
}

#[tokio::test]
async fn length_buckets_ignore_finishes_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(100)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;

    let buckets = length_buckets(&pool, user, T0 + 1).await.unwrap();

    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 0);
}

#[tokio::test]
async fn length_buckets_total_matches_the_finished_count_for_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Two measurable and one not: the chart must still account for all three,
    // or it describes fewer books than the Finished tile beside it.
    seed_book(&pool, lib, "uuid-a", None, Some(100)).await;
    seed_book(&pool, lib, "uuid-b", None, Some(600)).await;
    seed_book(&pool, lib, "uuid-c", None, None).await;
    for uuid in ["uuid-a", "uuid-b", "uuid-c"] {
        finish_journal(&pool, user, uuid, T0).await;
    }

    let buckets = length_buckets(&pool, user, 0).await.unwrap();

    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 3);
}

#[tokio::test]
async fn length_buckets_propagate_sqlx_error_when_the_books_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let err = length_buckets(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}

#[test]
fn bucket_case_sql_maps_null_to_unknown_and_falls_through_to_the_open_bucket() {
    let sql = bucket_case_sql("p.pages");
    assert!(sql.starts_with("CASE WHEN p.pages IS NULL THEN 3 "));
    assert!(sql.contains("WHEN p.pages < 300 THEN 0 "));
    assert!(sql.contains("WHEN p.pages < 500 THEN 1 "));
    // The fall-through is the bucket with no upper bound, wherever it sits in
    // the array — not "the last index".
    let open = LENGTH_BUCKETS
        .iter()
        .position(|(_, upper)| upper.is_none())
        .expect("one bucket must be open-ended");
    assert!(sql.ends_with(&format!("ELSE {open} END")), "{sql}");
}
