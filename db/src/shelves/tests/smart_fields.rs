//! The remaining smart-shelf rule fields: date-added against the epoch
//! column, author and series matching, membership updating as books
//! appear, the rating and read-status rules resolving against the shelf
//! owner, and `preview_rule`'s matched/total report.

use omnibus_shared::{MatchMode, RuleField, RuleOp, ShelfRule, SortDir, SortKey};

use super::super::*;
use super::{make_user, smart_req, tag_rule, uuid_by_title};
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

#[tokio::test]
async fn smart_shelf_date_added_rules_match_epoch_column() {
    // `books.timestamp` is INTEGER unix-seconds (migration 0038); the date-rule
    // SQL must compare it as an epoch (`date(col,'unixepoch')`, numeric
    // `strftime('%s',…)`) rather than as a TEXT date, or every match silently
    // returns nothing.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let owner = make_user(&pool, "owner", false).await;
    sqlx::query(
        "UPDATE books SET timestamp = strftime('%s','2024-06-15 00:00:00') \
                 WHERE id = (SELECT MIN(id) FROM books)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE books SET timestamp = strftime('%s','2020-01-01 00:00:00') \
                 WHERE id = (SELECT MAX(id) FROM books)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let date_rule = |op, value: &str| ShelfRule {
        field: RuleField::DateAdded,
        op,
        value: value.into(),
    };

    // `After` an absolute date → only the 2024 book.
    let after = create_shelf(
        &pool,
        owner,
        &smart_req(
            "After",
            MatchMode::Any,
            vec![date_rule(RuleOp::After, "2024-01-01")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        after.book_count, 1,
        "only the 2024-added book is after 2024-01-01"
    );

    // `Between` a calendar window → the same single book.
    let between = create_shelf(
        &pool,
        owner,
        &smart_req(
            "June",
            MatchMode::Any,
            vec![date_rule(RuleOp::Between, "2024-06-01..2024-06-30")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        between.book_count, 1,
        "only the mid-June book is in the window"
    );

    // `InLast 1d` exercises the numeric epoch comparison — both books are years
    // old, so it must match none (a TEXT/INTEGER mismatch here would misbehave).
    let recent = create_shelf(
        &pool,
        owner,
        &smart_req(
            "Recent",
            MatchMode::Any,
            vec![date_rule(RuleOp::InLast, "1d")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(recent.book_count, 0, "no book was added in the last day");
}

#[tokio::test]
async fn smart_shelf_matches_author_by_name_case_insensitively() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // "Ada Lovelace" authored 3 of the 4 fixture books. A lowercase value must
    // still match — regression: `author is` used to demand a numeric id, so a
    // typed name (any case) matched nothing.
    let rule = ShelfRule {
        field: RuleField::Author,
        op: RuleOp::Is,
        value: "ada lovelace".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("By Ada", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 3);
}

#[tokio::test]
async fn smart_shelf_matches_series_starts_with() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // Series "Saga" holds two books; a `starts with` prefix matches both.
    let rule = ShelfRule {
        field: RuleField::Series,
        op: RuleOp::StartsWith,
        value: "Sag".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Saga-ish", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 2);
}

#[tokio::test]
async fn smart_shelf_updates_when_a_qualifying_book_appears() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Essays", MatchMode::Any, vec![tag_rule("essay")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);

    // Tag another existing book "essay"; membership recomputes on next read.
    let book = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE title = 'Standalone'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let _ = book; // Standalone already has "essay"; tag a second book instead.
    let other = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE title = 'Other Story'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = 'essay'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(other)
        .bind(tag_id)
        .execute(&pool)
        .await
        .unwrap();

    let reloaded = get_shelf(&pool, shelf.id).await.unwrap().unwrap();
    assert_eq!(reloaded.book_count, 2);
}

#[tokio::test]
async fn rating_rule_resolves_against_shelf_owner() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let other = make_user(&pool, "other", false).await;
    let saga = uuid_by_title(&pool, "Saga: Book One").await;

    // Owner rates it 5★; the other user rates a different book 5★.
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?, ?, 10)")
        .bind(owner)
        .bind(&saga)
        .execute(&pool)
        .await
        .unwrap();
    let standalone = uuid_by_title(&pool, "Standalone").await;
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?, ?, 10)")
        .bind(other)
        .bind(&standalone)
        .execute(&pool)
        .await
        .unwrap();

    let rule = ShelfRule {
        field: RuleField::Rating,
        op: RuleOp::Gte,
        value: "4".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Top rated", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    // Only the owner's 5★ book qualifies — the other user's rating is invisible.
    assert_eq!(shelf.book_count, 1);
    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books[0].title.as_deref(), Some("Saga: Book One"));
}

#[tokio::test]
async fn status_rule_resolves_against_shelf_owner() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let other = make_user(&pool, "other", false).await;
    let saga = uuid_by_title(&pool, "Saga: Book One").await;
    let standalone = uuid_by_title(&pool, "Standalone").await;

    // Owner finishes Saga; the other user finishes Standalone.
    let finish = |user: i64, uuid: String| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO book_read_status (user_id, book_uuid, status, finished_at)
                 VALUES (?, ?, 'finished', strftime('%s','now'))",
            )
            .bind(user)
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    finish(owner, saga).await;
    finish(other, standalone).await;

    let rule = ShelfRule {
        field: RuleField::Status,
        op: RuleOp::Is,
        value: "finished".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Finished", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    // Only the owner's finished book qualifies — the other user's is invisible.
    assert_eq!(shelf.book_count, 1);
    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books[0].title.as_deref(), Some("Saga: Book One"));
}

#[tokio::test]
async fn unread_status_rule_matches_books_with_no_row() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let saga = uuid_by_title(&pool, "Saga: Book One").await;

    // Finish exactly one book; every other fixture book is unread (no row).
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, finished_at)
         VALUES (?, ?, 'finished', strftime('%s','now'))",
    )
    .bind(owner)
    .bind(&saga)
    .execute(&pool)
    .await
    .unwrap();

    let rule = ShelfRule {
        field: RuleField::Status,
        op: RuleOp::Is,
        value: "unread".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("To read", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    // The finished book is excluded; the rest (no row) all count as unread.
    assert!(shelf.book_count >= 1);
    assert!(
        !page
            .books
            .iter()
            .any(|b| b.title.as_deref() == Some("Saga: Book One")),
        "finished book must not appear in the unread shelf"
    );
}

#[tokio::test]
async fn preview_rule_reports_matched_and_total() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let preview = preview_rule(&pool, owner, MatchMode::Any, &[tag_rule("fiction")])
        .await
        .unwrap();
    assert_eq!(preview.matched, 2);
    assert_eq!(preview.total, 4);
    assert_eq!(preview.sample.len(), 2);
}
