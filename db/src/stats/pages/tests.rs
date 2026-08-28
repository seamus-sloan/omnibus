//! Unit tests for the length ladder and the two aggregates over it,
//! [`super::pages_read`] and [`super::length_buckets`]. Every input is a
//! persisted column, so these seed those columns directly — no EPUB or CBZ is
//! opened at query time.

use super::*;
use crate::init_db;

const T0: i64 = 1_700_000_000;

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
    finish_journal(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(412));
}

#[tokio::test]
async fn pages_read_uses_the_comic_page_count_when_no_print_pages_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A CBZ carries an exact image-page count and no word count.
    seed_book(&pool, lib, "uuid-a", None, Some(32)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(32));
}

#[tokio::test]
async fn pages_read_falls_back_to_the_word_count_estimate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(275), None).await; // 1 page
    seed_book(&pool, lib, "uuid-b", Some(550), None).await; // 2 pages
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_journal(&pool, user, "uuid-b", T0).await;

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
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_journal(&pool, user, "uuid-b", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(1));
}

// --- pages_read ---------------------------------------------------------

#[tokio::test]
async fn pages_read_is_none_when_nothing_finished_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A finished-count-eligible book exists but is never marked finished.
    seed_book(&pool, lib, "uuid-a", Some(550), None).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_counts_a_book_finished_via_read_status() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(825), None).await; // 3 pages
                                                            // Finished via read-status only (no journal entry) — the tile uses the
                                                            // same unified completion definition as the rest of the stats page.
    finish_read_status(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(3));
}

#[tokio::test]
async fn pages_read_counts_a_book_finished_both_ways_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(550), None).await; // 2 pages
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_read_status(&pool, user, "uuid-a", T0).await;

    // The DISTINCT-book scope collapses the two completion events to one row,
    // so the length is counted once (2 pages, not 4).
    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(2));
}

#[tokio::test]
async fn pages_read_is_none_when_no_finished_book_resolves_a_length() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Finished, but every rung is NULL (audio-only / not-yet-backfilled).
    seed_book(&pool, lib, "uuid-a", None, None).await;
    finish_journal(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_finishes_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(550), None).await;
    finish_journal(&pool, user, "uuid-a", T0).await;

    // A window starting after the finish must not see it.
    assert_eq!(pages_read(&pool, user, T0 + 1).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_a_ghosted_book_with_no_live_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // Completion event references a uuid with no `books` row (ghosted): the
    // inner join drops it, so there is no data.
    finish_journal(&pool, user, "uuid-ghost", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
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
    // The open-ended bucket has no bound of its own, so it's the fall-through.
    assert!(sql.ends_with("ELSE 2 END"));
}
