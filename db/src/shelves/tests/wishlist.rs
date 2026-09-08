//! The built-in Wishlist system shelf: idempotent provisioning named after
//! its owner (display name when set), membership sourced from
//! `wishlist_entries` including fileless entries, public browsing, and the
//! rejections every mutating shelf call returns for it.

use omnibus_shared::physical::WishlistSource;
use omnibus_shared::{
    CreateShelfRequest, ShelfKind, SortDir, SortKey, UpdateShelfRequest, Visibility,
};

use super::super::*;
use super::{make_user, wishlist_shelf_id};
use crate::physical::add_wishlist_entry;
use crate::pool::init_db;
use crate::test_support::seed_minimal_books;

#[tokio::test]
async fn provision_wishlist_shelf_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader", false).await;

    provision_wishlist_shelf(&pool, user).await.unwrap();
    provision_wishlist_shelf(&pool, user).await.unwrap();

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM shelves WHERE owner_user_id = ? AND kind = 'wishlist'",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "exactly one wishlist shelf per user (AC1)");
}

#[tokio::test]
async fn wishlist_shelf_is_public_and_named_after_its_owner() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;

    let shelf = get_shelf(&pool, id).await.unwrap().unwrap();
    assert_eq!(shelf.kind, ShelfKind::Wishlist);
    assert_eq!(shelf.name, "reader's Wishlist");
    assert_eq!(shelf.visibility, Visibility::Public);
}

/// Provisioning is inline in the insert transaction, so the name query must see
/// the uncommitted `users` row.
#[tokio::test]
async fn create_user_provisions_a_wishlist_shelf_named_after_the_new_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = crate::auth::create_user(&pool, "newcomer", "correct-horse-battery")
        .await
        .unwrap();

    let shelf = get_shelf(&pool, wishlist_shelf_id(&pool, user.id).await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shelf.name, "newcomer's Wishlist");
}

#[tokio::test]
async fn provision_wishlist_shelves_names_each_backfilled_shelf_after_its_owner() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;

    crate::shelves::provision_wishlist_shelves(&pool)
        .await
        .unwrap();

    for (user, expected) in [(alice, "alice's Wishlist"), (bob, "bob's Wishlist")] {
        let shelf = get_shelf(&pool, wishlist_shelf_id(&pool, user).await)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(shelf.name, expected);
    }
}

#[tokio::test]
async fn wishlist_shelf_count_and_page_come_from_wishlist_entries() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;
    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();
    add_wishlist_entry(&pool, user, "uuid-2", WishlistSource::Scan)
        .await
        .unwrap();

    let shelf = get_shelf(&pool, id).await.unwrap().unwrap();
    assert_eq!(shelf.book_count, 2);

    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 2);
}

#[tokio::test]
async fn wishlist_shelf_shows_fileless_entries_hidden_from_all_books() {
    let _covers = crate::test_support::CoversTempDir::new("wishlist_fileless");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;
    // A wishlist-only, fileless book: hidden from All Books, but must appear
    // inside the wishlist shelf (AC4).
    let uuid = crate::physical::create_fileless_book(
        &pool,
        crate::physical::FilelessBook {
            title: "Someday".into(),
            authors: vec!["Ada Lovelace".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    add_wishlist_entry(&pool, user, &uuid, WishlistSource::Detail)
        .await
        .unwrap();

    let shelf = get_shelf(&pool, id).await.unwrap().unwrap();
    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 1);
    assert_eq!(page.books[0].title.as_deref(), Some("Someday"));
}

#[tokio::test]
async fn wishlist_shelf_membership_tracks_add_and_fulfillment() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;

    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();
    assert_eq!(get_shelf(&pool, id).await.unwrap().unwrap().book_count, 1);

    // Checking in a copy fulfills (clears) the wishlist entry — the shelf count
    // drops with no manual step (AC3).
    crate::physical::add_physical_copy(&pool, "uuid-1", None, Some(user), None)
        .await
        .unwrap();
    assert_eq!(get_shelf(&pool, id).await.unwrap().unwrap().book_count, 0);
}

#[tokio::test]
async fn another_user_can_browse_the_public_wishlist_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    let alice_shelf = wishlist_shelf_id(&pool, alice).await;
    provision_wishlist_shelf(&pool, bob).await.unwrap();
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    // Bob (non-admin) sees Alice's wishlist in his visible list (AC4).
    let visible = list_visible_shelves(&pool, bob, false).await.unwrap();
    assert!(
        visible.iter().any(|s| s.id == alice_shelf),
        "public wishlist shelf must be visible to another user"
    );
}

#[tokio::test]
async fn update_shelf_rejects_the_wishlist_system_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;

    let req = UpdateShelfRequest {
        name: Some("Renamed".into()),
        ..Default::default()
    };
    let err = update_shelf(&pool, id, &req).await.unwrap_err();
    assert!(matches!(err, ShelfError::SystemShelf));
}

#[tokio::test]
async fn delete_shelf_rejects_the_wishlist_system_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;

    let err = delete_shelf(&pool, id).await.unwrap_err();
    assert!(matches!(err, ShelfError::SystemShelf));
}

#[tokio::test]
async fn add_and_remove_book_reject_the_wishlist_system_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = make_user(&pool, "reader", false).await;
    let id = wishlist_shelf_id(&pool, user).await;

    let add_err = add_books(&pool, id, &["uuid-1".into()], user)
        .await
        .unwrap_err();
    assert!(matches!(add_err, ShelfError::SystemShelf));
    let rm_err = remove_book(&pool, id, "uuid-1").await.unwrap_err();
    assert!(matches!(rm_err, ShelfError::SystemShelf));
}

#[tokio::test]
async fn create_shelf_rejects_wishlist_kind() {
    let req = CreateShelfRequest {
        kind: ShelfKind::Wishlist,
        name: "Wishlist".into(),
        description: None,
        visibility: Visibility::Public,
        match_mode: None,
        rules: Vec::new(),
        book_uuids: Vec::new(),
    };
    assert!(
        req.validate().is_err(),
        "forging a wishlist shelf is rejected"
    );
}

#[tokio::test]
async fn wishlist_shelf_is_named_after_the_display_name_when_one_is_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "cool-guy-7", false).await;
    crate::auth::set_display_name(&pool, user, Some("Seamus"))
        .await
        .unwrap();

    // A shelf provisioned *after* the name is set composes from it directly,
    // rather than relying on `set_display_name`'s rename.
    crate::shelves::provision_wishlist_shelves(&pool)
        .await
        .unwrap();

    let shelf = get_shelf(&pool, wishlist_shelf_id(&pool, user).await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shelf.name, "Seamus's Wishlist");
}
