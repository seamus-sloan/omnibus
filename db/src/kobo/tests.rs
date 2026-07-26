use omnibus_shared::{CreateShelfRequest, MatchMode, RuleField, RuleOp, ShelfKind, ShelfRule};

use super::*;
use crate::init_db;
use crate::shelves::{create_shelf, update_shelf};
use crate::test_support::seed_synced_ebook;

async fn make_user(pool: &SqlitePool, username: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin) VALUES (?, 'x', 0) RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Create a hand-picked shelf holding `uuids` and flag it for Kobo sync.
async fn synced_manual_shelf(pool: &SqlitePool, owner: i64, name: &str, uuids: &[String]) -> i64 {
    let shelf = create_shelf(
        pool,
        owner,
        &CreateShelfRequest {
            kind: ShelfKind::Manual,
            name: name.into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: Vec::new(),
            book_uuids: uuids.to_vec(),
        },
    )
    .await
    .unwrap();
    set_sync(pool, shelf.id, true).await;
    shelf.id
}

async fn set_sync(pool: &SqlitePool, shelf_id: i64, on: bool) {
    update_shelf(
        pool,
        shelf_id,
        &omnibus_shared::UpdateShelfRequest {
            sync_to_kobo: Some(on),
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn sync_books_returns_books_from_an_opted_in_shelf_with_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    synced_manual_shelf(&pool, user, "Kobo", &[uuid.clone()]).await;

    let rows = sync_books(&pool, user).await.unwrap();

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.uuid, uuid);
    assert_eq!(row.title, "Dune");
    assert_eq!(row.author, "Frank Herbert");
}

#[tokio::test]
async fn sync_books_returns_empty_for_empty_library() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;

    let rows = sync_books(&pool, user).await.unwrap();

    assert!(rows.is_empty());
}

#[tokio::test]
async fn sync_books_excludes_books_that_are_on_no_opted_in_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let synced = seed_synced_ebook(&pool, "in.epub", "Included", "A").await;
    let unshelved = seed_synced_ebook(&pool, "out.epub", "Unshelved", "A").await;
    let unflagged = seed_synced_ebook(&pool, "off.epub", "Unflagged", "A").await;

    synced_manual_shelf(&pool, user, "Kobo", &[synced.clone()]).await;
    // A second shelf holding `unflagged`, deliberately left opted out.
    create_shelf(
        &pool,
        user,
        &CreateShelfRequest {
            kind: ShelfKind::Manual,
            name: "Not synced".into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: Vec::new(),
            book_uuids: vec![unflagged.clone()],
        },
    )
    .await
    .unwrap();

    let uuids: Vec<_> = sync_books(&pool, user)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.uuid)
        .collect();

    assert_eq!(uuids, vec![synced]);
    assert!(!uuids.contains(&unshelved));
    assert!(!uuids.contains(&unflagged));
}

#[tokio::test]
async fn sync_books_enumerates_every_opted_in_book_with_no_cap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let mut uuids = Vec::new();
    for i in 0..150 {
        uuids.push(
            seed_synced_ebook(
                &pool,
                &format!("book-{i}.epub"),
                &format!("Title {i}"),
                "Author",
            )
            .await,
        );
    }
    synced_manual_shelf(&pool, user, "Everything", &uuids).await;

    let rows = sync_books(&pool, user).await.unwrap();

    // Deliberately never adopts Calibre-Web's SYNC_ITEM_LIMIT=100 cap — and
    // notably not the shelf read path's MAX_BOOKS_RETURNED=500 either.
    assert_eq!(rows.len(), 150);
}

#[tokio::test]
async fn sync_books_delivers_exactly_one_book_for_a_single_book_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let wanted = seed_synced_ebook(&pool, "one.epub", "Just This One", "A").await;
    seed_synced_ebook(&pool, "two.epub", "Not This", "A").await;
    synced_manual_shelf(&pool, user, "Single", &[wanted.clone()]).await;

    let rows = sync_books(&pool, user).await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uuid, wanted);
}

#[tokio::test]
async fn sync_books_never_returns_another_users_opted_in_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice").await;
    let bob = make_user(&pool, "bob").await;
    let hers = seed_synced_ebook(&pool, "hers.epub", "Hers", "A").await;
    let his = seed_synced_ebook(&pool, "his.epub", "His", "B").await;
    synced_manual_shelf(&pool, alice, "Alice Kobo", &[hers.clone()]).await;
    synced_manual_shelf(&pool, bob, "Bob Kobo", &[his.clone()]).await;

    let alice_uuids: Vec<_> = sync_books(&pool, alice)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.uuid)
        .collect();

    assert_eq!(alice_uuids, vec![hers]);
    assert!(!alice_uuids.contains(&his));
}

#[tokio::test]
async fn sync_books_reflects_a_shelf_being_opted_out_again() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "A").await;
    let shelf_id = synced_manual_shelf(&pool, user, "Kobo", &[uuid]).await;
    assert_eq!(sync_books(&pool, user).await.unwrap().len(), 1);

    set_sync(&pool, shelf_id, false).await;

    assert!(sync_books(&pool, user).await.unwrap().is_empty());
}

#[tokio::test]
async fn sync_books_includes_smart_shelf_membership() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let shelf = create_shelf(
        &pool,
        user,
        &CreateShelfRequest {
            kind: ShelfKind::Smart,
            name: "By Herbert".into(),
            description: None,
            visibility: Default::default(),
            match_mode: Some(MatchMode::Any),
            rules: vec![ShelfRule {
                field: RuleField::Author,
                op: RuleOp::Is,
                value: "Frank Herbert".into(),
            }],
            book_uuids: Vec::new(),
        },
    )
    .await
    .unwrap();
    set_sync(&pool, shelf.id, true).await;

    let rows = sync_books(&pool, user).await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uuid, uuid);
}

#[tokio::test]
async fn sync_books_deduplicates_a_book_on_two_opted_in_shelves() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "A").await;
    synced_manual_shelf(&pool, user, "First", &[uuid.clone()]).await;
    synced_manual_shelf(&pool, user, "Second", &[uuid.clone()]).await;

    let rows = sync_books(&pool, user).await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uuid, uuid);
}

#[tokio::test]
async fn sync_books_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    pool.close().await;

    assert!(matches!(
        sync_books(&pool, user).await,
        Err(KoboError::Sqlx(_))
    ));
}

#[tokio::test]
async fn book_for_sync_returns_the_row_for_a_known_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let row = book_for_sync(&pool, &uuid).await.unwrap().unwrap();

    assert_eq!(row.uuid, uuid);
    assert_eq!(row.title, "Dune");
    assert_eq!(row.author, "Frank Herbert");
}

#[tokio::test]
async fn book_for_sync_returns_none_for_an_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    assert!(book_for_sync(&pool, "no-such-uuid")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn book_for_sync_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;

    assert!(matches!(
        book_for_sync(&pool, "any").await,
        Err(KoboError::Sqlx(_))
    ));
}
