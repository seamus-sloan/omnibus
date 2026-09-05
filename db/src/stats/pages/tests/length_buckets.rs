//! `length_buckets`: finished books sorted by resolved length, the
//! unknown bucket for an unmeasurable book, boundary pages in the upper
//! bucket, zero-filled buckets, one count per book, window scoping, the
//! total matching the finished count, and the bucket CASE SQL.

use super::super::*;
use super::{finish_journal, finish_read_status, seed_book, seed_lib, set_print_pages, T0};
use crate::init_db;
use crate::test_support::seed_user;

/// The count in a named bucket, or -1 when the bucket is missing entirely.
fn books_in(buckets: &[LengthBucket], label: &str) -> i64 {
    buckets
        .iter()
        .find(|b| b.label == label)
        .map(|b| b.books)
        .unwrap_or(-1)
}

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
