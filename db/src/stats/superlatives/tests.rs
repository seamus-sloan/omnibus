//! Unit tests for the window's most-X figures. Each superlative is a ranked
//! query, so what these pin is mostly what gets *excluded* and how ties break
//! — a superlative that silently crowns the wrong row still looks like a
//! finding.

use super::*;
use crate::init_db;
use crate::test_support::seed_user;

const DAY: i64 = 86_400;

/// 2023-11-14 00:00:00 UTC. Day-aligned on purpose: a session at `D0 + n` and
/// the `date(started_at, 'unixepoch')` bucket it lands in can't disagree.
const D0: i64 = 19_675 * DAY;

async fn seed_lib(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One `books` row. `page_count` is the CBZ rung of the length ladder — the
/// simplest way to give a book an exact, assertable length.
async fn seed_book(pool: &SqlitePool, lib_id: i64, uuid: &str, title: &str, pages: Option<i64>) {
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, page_count) VALUES (?,?,'',?,?)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(title)
    .bind(pages)
    .execute(pool)
    .await
    .unwrap();
}

async fn finish(pool: &SqlitePool, user: i64, uuid: &str, finished_at: i64) {
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

// --- the empty window ---------------------------------------------------

#[tokio::test]
async fn superlatives_are_all_absent_for_a_window_with_no_activity() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let s = superlatives(&pool, user, 0).await.unwrap();

    assert!(s.is_empty(), "{s:?}");
}

// --- longest / shortest book --------------------------------------------

#[tokio::test]
async fn extreme_books_name_the_two_ends_of_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    for (uuid, title, pages) in [
        ("u-short", "Novella", 90),
        ("u-mid", "Middling", 300),
        ("u-long", "Doorstopper", 900),
    ] {
        seed_book(&pool, lib, uuid, title, Some(pages)).await;
        finish(&pool, user, uuid, D0).await;
    }

    let s = superlatives(&pool, user, 0).await.unwrap();

    let longest = s.longest_book.unwrap();
    assert_eq!(longest.title, "Doorstopper");
    assert_eq!(longest.value, 900);
    let shortest = s.shortest_book.unwrap();
    assert_eq!(shortest.title, "Novella");
    assert_eq!(shortest.value, 90);
}

#[tokio::test]
async fn shortest_book_is_absent_when_the_window_finished_only_one_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "Only Book", Some(300)).await;
    finish(&pool, user, "u-a", D0).await;

    let s = superlatives(&pool, user, 0).await.unwrap();

    // The one book is genuinely the longest; calling it the shortest too
    // dresses a single datum as a range.
    assert_eq!(s.longest_book.unwrap().title, "Only Book");
    assert!(s.shortest_book.is_none());
}

#[tokio::test]
async fn shortest_book_is_absent_when_every_finished_book_is_the_same_length() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    for (uuid, title) in [("u-a", "Alpha"), ("u-b", "Beta")] {
        seed_book(&pool, lib, uuid, title, Some(300)).await;
        finish(&pool, user, uuid, D0).await;
    }

    let s = superlatives(&pool, user, 0).await.unwrap();

    assert_eq!(s.longest_book.unwrap().value, 300);
    assert!(s.shortest_book.is_none());
}

#[tokio::test]
async fn extreme_books_exclude_a_book_the_length_ladder_cannot_measure() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-book", "Measured", Some(300)).await;
    // An audiobook has no page analogue at all. Sorting its NULL as zero would
    // crown it the shortest book of the year.
    seed_book(&pool, lib, "u-audio", "Unmeasured", None).await;
    finish(&pool, user, "u-book", D0).await;
    finish(&pool, user, "u-audio", D0).await;

    let s = superlatives(&pool, user, 0).await.unwrap();

    assert_eq!(s.longest_book.unwrap().title, "Measured");
    // Only one measurable book survives, so the pair collapses to one figure.
    assert!(s.shortest_book.is_none());
}

#[tokio::test]
async fn extreme_books_break_a_length_tie_on_title() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    for (uuid, title, pages) in [
        ("u-b", "Beta", 500),
        ("u-a", "Alpha", 500),
        ("u-c", "Gamma", 100),
    ] {
        seed_book(&pool, lib, uuid, title, Some(pages)).await;
        finish(&pool, user, uuid, D0).await;
    }

    let s = superlatives(&pool, user, 0).await.unwrap();

    // Two books tie at 500; the answer must be the same on every run.
    assert_eq!(s.longest_book.unwrap().title, "Alpha");
    assert_eq!(s.shortest_book.unwrap().title, "Gamma");
}

#[tokio::test]
async fn extreme_books_ignore_finishes_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "Old", Some(300)).await;
    finish(&pool, user, "u-a", D0).await;

    let s = superlatives(&pool, user, D0 + 1).await.unwrap();

    assert!(s.longest_book.is_none());
}

#[test]
fn drop_degenerate_shortest_keeps_a_genuine_pair_and_drops_the_rest() {
    let sup = |uuid: &str, value| BookSuperlative {
        book_uuid: uuid.to_string(),
        title: uuid.to_string(),
        author: None,
        value,
    };
    // Different book, different length — a real range.
    assert!(drop_degenerate_shortest(Some(&sup("a", 500)), Some(sup("b", 100))).is_some());
    // Same book.
    assert!(drop_degenerate_shortest(Some(&sup("a", 500)), Some(sup("a", 500))).is_none());
    // Different books, identical length.
    assert!(drop_degenerate_shortest(Some(&sup("a", 300)), Some(sup("b", 300))).is_none());
    // Nothing to compare against.
    assert!(drop_degenerate_shortest(None, Some(sup("b", 100))).is_none());
}

// --- biggest day --------------------------------------------------------

#[tokio::test]
async fn biggest_day_sums_reading_and_listening_on_the_same_calendar_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "Book", Some(100)).await;
    read_session(&pool, user, "u-a", D0, 3600).await;
    // Day two is bigger only once both formats are counted — the metric has to
    // agree with every other activity figure on the page about that.
    read_session(&pool, user, "u-a", D0 + DAY, 2000).await;
    listen_session(&pool, user, "u-a", D0 + DAY + 60, 2000).await;

    let day = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .biggest_day
        .unwrap();

    assert_eq!(day.day, "2023-11-15");
    assert_eq!(day.seconds, 4000);
}

#[tokio::test]
async fn biggest_day_breaks_a_tie_on_the_earliest_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "Book", Some(100)).await;
    read_session(&pool, user, "u-a", D0 + DAY, 3600).await;
    read_session(&pool, user, "u-a", D0, 3600).await;

    let day = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .biggest_day
        .unwrap();

    assert_eq!(day.day, "2023-11-14");
}

// --- longest sit --------------------------------------------------------

#[tokio::test]
async fn longest_sit_stitches_checkpoints_rather_than_ranking_raw_rows() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-marathon", "Marathon", Some(100)).await;
    seed_book(&pool, lib, "u-single", "Single Row", Some(100)).await;
    // Three back-to-back checkpoints — one 45-minute sitting, not three
    // 15-minute ones. Ranking raw rows would crown the single 20-minute row.
    for i in 0..3 {
        read_session(&pool, user, "u-marathon", D0 + i * 900, 900).await;
    }
    read_session(&pool, user, "u-single", D0 + 10 * DAY, 1200).await;

    let sit = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .longest_sit
        .unwrap();

    assert_eq!(sit.title, "Marathon");
    assert_eq!(sit.value, 2700);
}

#[tokio::test]
async fn longest_sit_ignores_a_glance_under_the_sitting_floor() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "u-a", "Glanced At", Some(100)).await;
    read_session(&pool, user, "u-a", D0, sessionize::MIN_SITTING_SECS - 1).await;

    assert!(superlatives(&pool, user, 0)
        .await
        .unwrap()
        .longest_sit
        .is_none());
}

#[tokio::test]
async fn longest_sit_ignores_a_ghosted_book_with_no_live_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // Sessions on a uuid with no `books` row: it can't be named, so it can't
    // be a superlative.
    read_session(&pool, user, "u-ghost", D0, 7200).await;

    assert!(superlatives(&pool, user, 0)
        .await
        .unwrap()
        .longest_sit
        .is_none());
}

// --- fastest read -------------------------------------------------------

/// A book with enough recorded time to clear the fastest-read floor, opened
/// `first_at` and finished `finished_at`.
async fn seed_raced_book(
    pool: &SqlitePool,
    lib: i64,
    user: i64,
    uuid: &str,
    title: &str,
    (first_at, finished_at): (i64, i64),
) {
    seed_book(pool, lib, uuid, title, Some(300)).await;
    read_session(pool, user, uuid, first_at, FASTEST_READ_MIN_SECS).await;
    finish(pool, user, uuid, finished_at).await;
}

#[tokio::test]
async fn fastest_read_counts_days_from_the_first_recorded_session() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_raced_book(&pool, lib, user, "u-fast", "Sprint", (D0, D0 + 3 * DAY)).await;
    seed_raced_book(&pool, lib, user, "u-slow", "Slog", (D0, D0 + 40 * DAY)).await;

    let fastest = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .fastest_read
        .unwrap();

    assert_eq!(fastest.title, "Sprint");
    assert_eq!(fastest.value, 3);
}

#[tokio::test]
async fn fastest_read_reports_a_same_day_read_as_one_day_not_zero() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_raced_book(&pool, lib, user, "u-a", "One Sitting", (D0, D0 + 3600)).await;

    let fastest = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .fastest_read
        .unwrap();

    // "Read in 0 days" is not a sentence about reading.
    assert_eq!(fastest.value, 1);
}

#[tokio::test]
async fn fastest_read_counts_reading_that_began_before_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Opened 30 days before the window, finished on its first day. Clipping
    // the sessions to the window would report this as a one-day sprint.
    seed_raced_book(&pool, lib, user, "u-a", "Long Haul", (D0 - 30 * DAY, D0)).await;

    let fastest = superlatives(&pool, user, D0)
        .await
        .unwrap()
        .fastest_read
        .unwrap();

    assert_eq!(fastest.value, 30);
}

#[tokio::test]
async fn fastest_read_ignores_a_book_under_the_tracked_time_floor() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Read on another device and marked finished here, leaving one stray
    // checkpoint on the finish day — the case the floor exists for.
    seed_book(&pool, lib, "u-elsewhere", "Read On A Kobo", Some(300)).await;
    read_session(&pool, user, "u-elsewhere", D0, FASTEST_READ_MIN_SECS - 1).await;
    finish(&pool, user, "u-elsewhere", D0).await;
    seed_raced_book(
        &pool,
        lib,
        user,
        "u-real",
        "Actually Raced",
        (D0, D0 + 4 * DAY),
    )
    .await;

    let fastest = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .fastest_read
        .unwrap();

    assert_eq!(fastest.title, "Actually Raced");
    assert_eq!(fastest.value, 4);
}

#[tokio::test]
async fn fastest_read_ignores_a_book_whose_sessions_all_postdate_its_completion() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Finished elsewhere first, re-read here afterwards: the span is negative,
    // which is not a fast read.
    seed_raced_book(&pool, lib, user, "u-a", "Reread", (D0 + 5 * DAY, D0)).await;

    assert!(superlatives(&pool, user, 0)
        .await
        .unwrap()
        .fastest_read
        .is_none());
}

#[tokio::test]
async fn fastest_read_uses_the_earliest_in_window_completion() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_raced_book(&pool, lib, user, "u-a", "Marked Twice", (D0, D0 + 2 * DAY)).await;
    // Re-marked finished a month later. Taking the newest completion would
    // stretch a two-day read into a month-long one.
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at)
         VALUES (?, 'u-a', 'again', 100, ?)",
    )
    .bind(user)
    .bind(D0 + 32 * DAY)
    .execute(&pool)
    .await
    .unwrap();

    let fastest = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .fastest_read
        .unwrap();

    assert_eq!(fastest.value, 2);
}

#[tokio::test]
async fn fastest_read_breaks_a_tie_on_title() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_raced_book(&pool, lib, user, "u-b", "Beta", (D0, D0 + 2 * DAY)).await;
    seed_raced_book(&pool, lib, user, "u-a", "Alpha", (D0, D0 + 2 * DAY)).await;

    let fastest = superlatives(&pool, user, 0)
        .await
        .unwrap()
        .fastest_read
        .unwrap();

    assert_eq!(fastest.title, "Alpha");
}

// --- error path ---------------------------------------------------------

#[tokio::test]
async fn superlatives_propagate_sqlx_error_when_the_books_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let err = superlatives(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}
