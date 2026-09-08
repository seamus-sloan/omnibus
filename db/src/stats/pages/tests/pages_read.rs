//! `pages_read` and its bounded variant: the length ladder (print pages,
//! comic page count, word estimate), what counts in a window (partial
//! books, ground covered, every book and day, one rounding), what does not
//! (other users, ghosted books, finishes covering no ground), the boundary
//! day, and the DB-failure path.

use super::super::*;
use super::{
    finish_journal, finish_read_status, read_percent, seed_book, seed_lib, set_print_pages, T0,
    T0_DAY,
};
use crate::init_db;
use crate::test_support::seed_user;

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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(412));
}

#[tokio::test]
async fn pages_read_uses_the_comic_page_count_when_no_print_pages_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A CBZ carries an exact image-page count and no word count.
    seed_book(&pool, lib, "uuid-a", None, Some(32)).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 100).await;

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(32));
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(32));
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(3));
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(1));
}

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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(100));
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), None);
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(150));
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(1));
}

#[tokio::test]
async fn pages_read_is_none_when_nothing_was_read_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(550), None).await;

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), None);
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

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_ground_covered_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-14", 50).await;

    // A window starting the next day must not see it.
    assert_eq!(pages_read(&pool, user, T0 + 86_400, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_another_users_reading() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    read_percent(&pool, bob, "uuid-a", T0_DAY, 50).await;

    assert_eq!(pages_read(&pool, alice, 0, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_a_ghosted_book_with_no_live_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // A ledger row against a uuid with no `books` row (ghosted): the join
    // drops it, so there is no data.
    read_percent(&pool, user, "uuid-ghost", T0_DAY, 60).await;

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_propagates_sqlx_error_when_the_ledger_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE reading_progress_daily")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_read(&pool, 1, 0, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}

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
    let pages = pages_read_bounded(&pool, user, T0, T0, 0).await.unwrap();

    assert_eq!(pages, 20);
}

/// Midnight UTC on 2023-11-15 — `compute::prev_window_from` clamps a baseline's
/// `end` to the current period's start, which is always a day boundary.
const T0_NEXT_MIDNIGHT: i64 = 1_700_006_400;

#[tokio::test]
async fn pages_read_bounded_excludes_a_boundary_landing_exactly_on_midnight() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(100)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-14", 20).await;
    read_percent(&pool, user, "uuid-a", "2023-11-15", 40).await;

    // `end` at midnight means zero elapsed seconds on the day it names, so that
    // day belongs to the *next* window. Counting it would let the clamped
    // baseline swallow day one of the window it is the baseline for, and report
    // a 0% delta against a day comparing with itself.
    let pages = pages_read_bounded(&pool, user, T0, T0_NEXT_MIDNIGHT, 0)
        .await
        .unwrap();

    assert_eq!(pages, 20);
}

#[tokio::test]
async fn pages_read_bounded_is_zero_rather_than_none_for_an_empty_baseline() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert_eq!(pages_read_bounded(&pool, user, 0, T0, 0).await.unwrap(), 0);
}
