//! Unit tests for [`super::rating_histogram`] and the window/liveness rules it
//! shares with [`super::avg_stars`]. The mean and the trailing-month trend keep
//! their coverage in the `stats/tests.rs` suite that drives them through
//! `compute`; these cover the distribution and the invariant tying it to the
//! mean.

use super::*;
use crate::init_db;
use crate::test_support::seed_minimal_books;

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

/// The bucket count for one half-star value.
fn books_at(buckets: &[RatingBucket], half_stars: i64) -> i64 {
    buckets
        .iter()
        .find(|b| b.half_stars == half_stars)
        .map(|b| b.books)
        .unwrap_or(-1)
}

#[test]
fn rating_bucket_reports_its_value_in_stars_not_half_stars() {
    let stars = |h| {
        RatingBucket {
            half_stars: h,
            books: 0,
        }
        .stars()
    };
    assert!((stars(1) - 0.5).abs() < f64::EPSILON);
    assert!((stars(7) - 3.5).abs() < f64::EPSILON);
    assert!((stars(10) - 5.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn rating_histogram_spreads_books_across_their_half_star_buckets() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 4).await;
    let user = seed_user(&pool, "alice").await;

    rate_book(&pool, user, "uuid-1", 10, T0).await; // 5.0
    rate_book(&pool, user, "uuid-2", 10, T0).await; // 5.0
    rate_book(&pool, user, "uuid-3", 7, T0).await; //  3.5
    rate_book(&pool, user, "uuid-4", 1, T0).await; //  0.5

    let buckets = rating_histogram(&pool, user, 0).await.unwrap();

    assert_eq!(books_at(&buckets, 10), 2);
    assert_eq!(books_at(&buckets, 7), 1);
    assert_eq!(books_at(&buckets, 1), 1);
    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 4);
}

#[tokio::test]
async fn rating_histogram_returns_all_ten_buckets_including_empty_ones() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    rate_book(&pool, user, "uuid-1", 8, T0).await;

    let buckets = rating_histogram(&pool, user, 0).await.unwrap();

    // A histogram with missing bars reads as a different distribution.
    assert_eq!(buckets.len(), 10);
    assert_eq!(
        buckets.iter().map(|b| b.half_stars).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
    assert_eq!(books_at(&buckets, 4), 0);
}

#[tokio::test]
async fn rating_histogram_is_all_zero_for_a_window_with_no_ratings() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    rate_book(&pool, user, "uuid-1", 8, T0).await;

    // Window opens after the rating: the spine still comes back, all zero, so
    // the caller distinguishes "no ratings" by the total rather than by length.
    let buckets = rating_histogram(&pool, user, T0 + 1).await.unwrap();

    assert_eq!(buckets.len(), 10);
    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 0);
}

#[tokio::test]
async fn rating_histogram_holds_one_book_for_a_single_rating_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    rate_book(&pool, user, "uuid-1", 6, T0).await;

    let buckets = rating_histogram(&pool, user, 0).await.unwrap();

    assert_eq!(books_at(&buckets, 6), 1);
    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 1);
}

#[tokio::test]
async fn rating_histogram_total_matches_the_ratings_the_mean_is_taken_over() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;
    // In window: 2 and 5 stars → mean 3.5, two books. Out of window: one more.
    rate_book(&pool, user, "uuid-1", 4, T0 + 100).await;
    rate_book(&pool, user, "uuid-2", 10, T0 + 100).await;
    rate_book(&pool, user, "uuid-3", 2, T0 - 100).await;

    let buckets = rating_histogram(&pool, user, T0).await.unwrap();
    let mean = avg_stars(&pool, user, T0).await.unwrap();

    assert_eq!(mean, Some(3.5));
    assert_eq!(
        buckets.iter().map(|b| b.books).sum::<i64>(),
        2,
        "the distribution must cover exactly the ratings the mean averages"
    );
}

#[tokio::test]
async fn rating_histogram_excludes_a_rating_on_a_ghosted_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    rate_book(&pool, user, "uuid-1", 8, T0).await;
    // No `books` row behind this uuid: the page can't render it, so it must
    // not move the distribution any more than it moves the mean.
    rate_book(&pool, user, "uuid-ghost", 2, T0).await;

    let buckets = rating_histogram(&pool, user, 0).await.unwrap();

    assert_eq!(books_at(&buckets, 8), 1);
    assert_eq!(books_at(&buckets, 2), 0);
    assert_eq!(buckets.iter().map(|b| b.books).sum::<i64>(), 1);
}

#[tokio::test]
async fn rating_histogram_is_scoped_to_the_requesting_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    rate_book(&pool, alice, "uuid-1", 10, T0).await;
    rate_book(&pool, bob, "uuid-2", 2, T0).await;

    let buckets = rating_histogram(&pool, alice, 0).await.unwrap();

    assert_eq!(books_at(&buckets, 10), 1);
    assert_eq!(books_at(&buckets, 2), 0);
}

#[tokio::test]
async fn rating_histogram_propagates_sqlx_error_when_the_ratings_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE user_ratings")
        .execute(&pool)
        .await
        .unwrap();

    let err = rating_histogram(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}
