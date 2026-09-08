//! Finished books: a completion comes from a 100% journal entry or an
//! explicit read status and counts once when both exist, the twelve-month
//! `books_per_month` spine, the rail's cover and rating fields and its cap,
//! and a completion that outlives its book.

use omnibus_shared::StatsRange;

use super::super::*;
use super::{
    book_id, drop_book_row, finish_journal, finish_read_status, link_author, months_ago_secs,
    rate_book, seed_user, this_month_secs, T0,
};
use crate::init_db;
use crate::test_support::seed_minimal_books;

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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.books_finished, 1);
    assert_eq!(s.finished_books.len(), 1);
    // The rail's finish time is the newest completion moment across sources.
    assert_eq!(s.finished_books[0].finished_at, T0 + 100);
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

    let months = books_per_month(&pool, user, 0).await.unwrap();

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

    let months = books_per_month(&pool, user, 0).await.unwrap();

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

    let months = books_per_month(&pool, user, 0).await.unwrap();
    assert_eq!(months.last().unwrap().books, 1);
}

#[tokio::test]
async fn books_per_month_is_empty_of_finishes_for_a_user_with_no_activity() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "loner").await;

    let months = books_per_month(&pool, user, 0).await.unwrap();
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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
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

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.finished_books[0].rating, Some(4.5));
}

#[tokio::test]
async fn finished_books_rating_is_none_when_unrated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    finish_journal(&pool, user, "uuid-1", T0).await;

    let s = compute(&pool, user, StatsRange::AllTime, 0).await.unwrap();
    assert_eq!(s.finished_books[0].rating, None);
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

#[tokio::test]
async fn finished_metrics_agree_when_a_completion_outlives_its_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    let at = this_month_secs(&pool).await;
    finish_read_status(&pool, user, "uuid-1", at).await;
    finish_read_status(&pool, user, "uuid-2", at).await;
    drop_book_row(&pool, "uuid-2").await;

    // The headline count, the rail and the trailing-12 chart are three reads
    // of one definition; before the shared liveness filter the chart reported
    // 2 while the tile above it reported 1, for the same month.
    let headline = finished_count(&pool, user, 0).await.unwrap();
    let rail = finished_books(&pool, user, 0).await.unwrap();
    let months = books_per_month(&pool, user, 0).await.unwrap();
    assert_eq!(headline, 1);
    assert_eq!(rail.len(), 1);
    assert_eq!(months.last().unwrap().books, 1);
}
