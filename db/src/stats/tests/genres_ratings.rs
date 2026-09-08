//! The genre donut and the rating figures: `genre_share` counts distinct
//! books per genre (tags ignored, case folded, window scoped, every genre
//! returned), `genre_tagged_books`, `avg_stars`, the twelve-month
//! `rating_monthly` spine, and a rating whose book is gone.

use omnibus_shared::StatsRange;
use sqlx::Row;

use super::super::*;
use super::{
    book_id, drop_book_row, link_tag, listening_session, months_ago_secs, prev_period_start,
    rate_book, reading_session, seed_user, set_genres, this_month_secs, DAY, T0,
};
use crate::init_db;
use crate::test_support::seed_minimal_books;

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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();

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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.avg_stars, None);
}

#[tokio::test]
async fn rating_monthly_returns_twelve_months_zeroed_when_unrated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    let months = rating_monthly(&pool, user, 0).await.unwrap();
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

    let months = rating_monthly(&pool, user, 0).await.unwrap();
    assert_eq!(months.last().unwrap().value, 3.5);
}

#[tokio::test]
async fn rating_monthly_cuts_its_spine_and_its_buckets_on_the_readers_calendar() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // UTC+13. The first instant of the reader's current month is always still
    // the previous month in UTC, so a UTC spine and a UTC join file this
    // rating a month early — the exact drift against `books_per_month` and
    // the chart builder that this covers.
    const OFFSET: i64 = 780;
    // Both off one clock read, so a month rollover between them can't flake.
    let row = sqlx::query(&format!(
        "SELECT CAST(strftime('%s', 'now', '+{OFFSET} minutes', 'start of month') AS INTEGER)
                    - {OFFSET} * 60 AS at,
                strftime('%Y-%m', 'now', '+{OFFSET} minutes') AS month"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    let (at, local_month): (i64, String) = (row.get("at"), row.get("month"));
    rate_book(&pool, user, "uuid-1", 7, at).await;

    let months = rating_monthly(&pool, user, OFFSET).await.unwrap();

    let last = months.last().unwrap();
    assert_eq!(last.label, local_month, "spine ends on the reader's month");
    assert!(
        (last.value - 3.5).abs() < f64::EPSILON,
        "the rating belongs to that month: {months:?}"
    );
}

#[tokio::test]
async fn avg_stars_excludes_a_rating_whose_book_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let at = this_month_secs(&pool).await;
    rate_book(&pool, user, "uuid-1", 10, at).await;
    rate_book(&pool, user, "uuid-2", 2, at).await;
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
    let at = this_month_secs(&pool).await;
    rate_book(&pool, user, "uuid-1", 10, at).await;
    rate_book(&pool, user, "uuid-2", 2, at).await;
    drop_book_row(&pool, "uuid-2").await;

    let months = rating_monthly(&pool, user, 0).await.unwrap();
    assert_eq!(months.len(), 12);
    // Same filter as `avg_stars`, so the tile and its trend agree.
    assert!(
        (months.last().unwrap().value - 5.0).abs() < f64::EPSILON,
        "expected the current month to mean 5.0, got {months:?}"
    );
}

#[tokio::test]
async fn genre_tagged_books_counts_each_active_tagged_book_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;
    // Three genres on one book, one on another, none on the third.
    set_genres(&pool, "uuid-1", &["Fantasy", "Horror", "Gothic"], user).await;
    set_genres(&pool, "uuid-2", &["Fantasy"], user).await;
    for uuid in ["uuid-1", "uuid-2", "uuid-3"] {
        reading_session(&pool, user, uuid, T0, 600).await;
    }

    // Two books carry a genre between them, even though they contribute four
    // slice entries — the donut's center must not read as the slice total.
    assert_eq!(genre_tagged_books(&pool, user, 0).await.unwrap(), 2);
    // …and the third active book is the untagged remainder the card discloses.
    assert_eq!(books_active(&pool, user, 0).await.unwrap(), 3);
}

#[tokio::test]
async fn genre_tagged_books_is_zero_when_no_active_book_has_a_genre() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    assert_eq!(genre_tagged_books(&pool, user, 0).await.unwrap(), 0);
}

#[tokio::test]
async fn genre_share_returns_every_genre_so_the_donut_can_size_its_tail() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Twelve genres on one book — more than the old `LIMIT 8`, which left the
    // donut folding "Other" over a truncated tail while its percentages still
    // summed to 100%.
    let names: Vec<String> = (1..=12).map(|i| format!("Genre {i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    set_genres(&pool, "uuid-1", &refs, user).await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let shares = genre_share(&pool, user, 0).await.unwrap();
    assert_eq!(shares.len(), 12);
    assert!(shares.iter().all(|s| s.books == 1));
}

#[tokio::test]
async fn previous_period_avg_stars_excludes_a_rating_whose_book_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let prev = prev_period_start(&pool, StatsRange::Month).await;
    rate_book(&pool, user, "uuid-1", 10, prev).await;
    rate_book(&pool, user, "uuid-2", 2, prev).await;
    drop_book_row(&pool, "uuid-2").await;

    // `avg_stars_bounded` carries its own copy of the filter; without this the
    // baseline mean could disagree with the current window's for the same books.
    let previous = previous_period(&pool, user, StatsRange::Month, 0)
        .await
        .unwrap();
    assert_eq!(previous.avg_stars, Some(5.0));
}
