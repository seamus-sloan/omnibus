//! Manual membership: insertion order, the unknown-book rejection,
//! add/remove, survival of a ghosted file, the chunked batch inserts for
//! books and rules, and `manual_shelves_containing`.

use omnibus_shared::{MatchMode, ShelfRule, SortDir, SortKey};

use super::super::*;
use super::{make_user, manual_req, smart_req, tag_rule, uuid_by_title};
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

#[tokio::test]
async fn create_manual_shelf_keeps_added_order() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let owner = make_user(&pool, "owner", false).await;

    let shelf = create_shelf(
        &pool,
        owner,
        &manual_req("Picks", vec!["uuid-3".into(), "uuid-1".into()]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 2);

    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    let titles: Vec<_> = page.books.iter().filter_map(|b| b.title.clone()).collect();
    assert_eq!(titles, vec!["Title 3", "Title 1"]); // position order, not sort
}

#[tokio::test]
async fn create_manual_shelf_rejects_unknown_book_before_inserting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let owner = make_user(&pool, "owner", false).await;

    let err = create_shelf(&pool, owner, &manual_req("Bad", vec!["nope".into()]))
        .await
        .unwrap_err();
    assert!(matches!(err, ShelfError::BookNotFound));
    // The shelf row must not have been created.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shelves")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn add_and_remove_book_updates_membership() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Picks", vec![]))
        .await
        .unwrap();

    add_books(&pool, shelf.id, &["uuid-1".into(), "uuid-2".into()], owner)
        .await
        .unwrap();
    assert_eq!(
        get_shelf(&pool, shelf.id)
            .await
            .unwrap()
            .unwrap()
            .book_count,
        2
    );

    remove_book(&pool, shelf.id, "uuid-1").await.unwrap();
    assert_eq!(
        get_shelf(&pool, shelf.id)
            .await
            .unwrap()
            .unwrap()
            .book_count,
        1
    );
}

#[tokio::test]
async fn manual_membership_survives_a_ghosted_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Picks", vec!["uuid-1".into()]))
        .await
        .unwrap();

    // Ghost the book: drop its file row, keep the `books` row (uuid preserved).
    sqlx::query(
        "DELETE FROM book_files WHERE book_id = (SELECT id FROM books WHERE uuid = 'uuid-1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let reloaded = get_shelf(&pool, shelf.id).await.unwrap().unwrap();
    assert_eq!(reloaded.book_count, 1, "ghosted book stays on the shelf");
    let page = shelf_page(&pool, &reloaded, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 1);
}

#[tokio::test]
async fn create_manual_shelf_batches_book_inserts_across_the_chunk_boundary() {
    // 250 uuids forces `insert_books` through two chunks (200 + 50); this
    // must not lose, duplicate, or misorder any row at the boundary.
    const N: i64 = 250;
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, N).await;
    let owner = make_user(&pool, "owner", false).await;

    let uuids: Vec<String> = (1..=N).map(|i| format!("uuid-{i}")).collect();
    let shelf = create_shelf(&pool, owner, &manual_req("Big", uuids.clone()))
        .await
        .unwrap();
    assert_eq!(shelf.book_count, N);

    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    let titles: Vec<_> = page.books.iter().filter_map(|b| b.title.clone()).collect();
    let expected: Vec<_> = (1..=N).map(|i| format!("Title {i}")).collect();
    assert_eq!(
        titles, expected,
        "insertion order (== position) survives the 200-row chunk boundary"
    );
}

#[tokio::test]
async fn create_smart_shelf_batches_rule_inserts_across_the_chunk_boundary() {
    // 250 rules forces `insert_rules` through two chunks (199 + 51); this
    // must not lose, duplicate, or misorder any rule at the boundary.
    const N: usize = 250;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;

    let rules: Vec<ShelfRule> = (0..N).map(|i| tag_rule(&format!("tag-{i}"))).collect();
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Many rules", MatchMode::Any, rules),
    )
    .await
    .unwrap();

    let values: Vec<String> =
        sqlx::query_scalar("SELECT value FROM shelf_rules WHERE shelf_id = ? ORDER BY position")
            .bind(shelf.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    let expected: Vec<_> = (0..N).map(|i| format!("tag-{i}")).collect();
    assert_eq!(
        values, expected,
        "rule order (== position) survives the 199-row chunk boundary"
    );
}

#[tokio::test]
async fn manual_shelves_containing_names_every_hand_picked_shelf_holding_the_book() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let uuid = uuid_by_title(&pool, "Saga: Book One").await;

    let holding = create_shelf(&pool, owner, &manual_req("Picks", vec![uuid.clone()]))
        .await
        .unwrap();
    let empty = create_shelf(&pool, owner, &manual_req("Later", vec![]))
        .await
        .unwrap();

    let ids = manual_shelves_containing(&pool, owner, false, &uuid)
        .await
        .unwrap();
    assert_eq!(ids, vec![holding.id]);
    assert!(!ids.contains(&empty.id));
}

#[tokio::test]
async fn manual_shelves_containing_skips_smart_shelves() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let uuid = uuid_by_title(&pool, "Saga: Book One").await;

    // A smart shelf can hold the book, but its membership is derived from a
    // rule — there is nothing for a checklist to toggle.
    create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();

    let ids = manual_shelves_containing(&pool, owner, false, &uuid)
        .await
        .unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn manual_shelves_containing_hides_another_users_private_shelf() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let stranger = make_user(&pool, "stranger", false).await;
    let uuid = uuid_by_title(&pool, "Saga: Book One").await;

    create_shelf(&pool, owner, &manual_req("Picks", vec![uuid.clone()]))
        .await
        .unwrap();

    let ids = manual_shelves_containing(&pool, stranger, false, &uuid)
        .await
        .unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn manual_shelves_containing_returns_nothing_for_an_unknown_uuid() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    let ids = manual_shelves_containing(&pool, owner, false, "not-a-book")
        .await
        .unwrap();
    assert!(ids.is_empty());
}
