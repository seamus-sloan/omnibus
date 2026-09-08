//! Per-user physical wishlist: add (idempotent), remove, the per-user
//! scoped list and its hard response cap, and the single-entry lookup.

use omnibus_shared::physical::WishlistSource;
use sqlx::SqlitePool;

use crate::test_support::{seed_minimal_books, CoversTempDir};

use super::super::*;
use super::{count, pool, seed_user};

#[tokio::test]
async fn add_wishlist_entry_returns_the_entry() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    let entry = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();

    assert_eq!(entry.user_id, user);
    assert_eq!(entry.book_uuid, "uuid-1");
    assert_eq!(entry.source, WishlistSource::Scan);
}

/// Migration 0085 widened the `source` CHECK to admit the title-search door
/// (#2247); without it this insert fails the constraint outright.
#[tokio::test]
async fn add_wishlist_entry_accepts_the_title_search_source() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    let entry = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Search)
        .await
        .unwrap();

    assert_eq!(entry.source, WishlistSource::Search);
    assert_eq!(
        get_wishlist_entry(&pool, user, "uuid-1")
            .await
            .unwrap()
            .map(|e| e.source),
        Some(WishlistSource::Search)
    );
}

#[tokio::test]
async fn add_wishlist_entry_errors_when_book_missing() {
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let err = add_wishlist_entry(&pool, user, "nope", WishlistSource::Scan)
        .await
        .unwrap_err();
    assert!(matches!(err, PhysicalError::BookNotFound));
}

#[tokio::test]
async fn add_wishlist_entry_is_idempotent() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    let first = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();
    // A second add keeps the original row (same id, original source).
    let second = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Manual)
        .await
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(second.source, WishlistSource::Scan);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM wishlist_entries").await,
        1
    );
}

#[tokio::test]
async fn remove_wishlist_entry_deletes_and_is_a_noop_when_absent() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;
    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    remove_wishlist_entry(&pool, user, "uuid-1").await.unwrap();
    assert!(list_wishlist(&pool, user).await.unwrap().is_empty());

    // Removing again is not an error.
    remove_wishlist_entry(&pool, user, "uuid-1").await.unwrap();
}

/// Raw bulk insert bypassing `add_wishlist_entry` — the CRUD helper resolves
/// the book uuid on every call, too slow for a 1500-row response-cap
/// fixture. `wishlist_entries` uniques on `(user_id, book_uuid)`, so each
/// row needs a distinct synthetic uuid; the soft-reference (no FK) means
/// these need not resolve to real `books` rows for this query-shape test.
async fn seed_wishlist_raw(pool: &SqlitePool, user_id: i64, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO wishlist_entries (user_id, book_uuid, added_at, source)
        SELECT ?, 'seed-uuid-' || i, i, 'manual' FROM n
        ",
    )
    .bind(count)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn list_wishlist_caps_response_at_hard_limit() {
    let pool = pool().await;
    let user = seed_user(&pool, "alice").await;
    let over_cap = LIST_WISHLIST_LIMIT + 500;
    seed_wishlist_raw(&pool, user, over_cap).await;

    let list = list_wishlist(&pool, user).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_WISHLIST_LIMIT,
        "list_wishlist must not return more than LIST_WISHLIST_LIMIT rows",
    );
}

#[tokio::test]
async fn list_wishlist_is_scoped_per_user() {
    let pool = pool().await;
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();
    add_wishlist_entry(&pool, bob, "uuid-2", WishlistSource::Scan)
        .await
        .unwrap();

    let alice_list = list_wishlist(&pool, alice).await.unwrap();
    assert_eq!(alice_list.len(), 1);
    assert_eq!(alice_list[0].book_uuid, "uuid-1");
}

// --- wishlist lookup -------------------------------------------------------

#[tokio::test]
async fn get_wishlist_entry_returns_the_entry_when_wishlisted() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;
    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    let entry = get_wishlist_entry(&pool, user, "uuid-1").await.unwrap();

    let entry = entry.expect("wishlisted book must return its entry");
    assert_eq!(entry.book_uuid, "uuid-1");
    assert_eq!(entry.source, WishlistSource::Detail);
}

#[tokio::test]
async fn get_wishlist_entry_returns_none_when_not_wishlisted() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    assert!(get_wishlist_entry(&pool, user, "uuid-1")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn get_wishlist_entry_is_scoped_to_the_asking_user() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    assert!(get_wishlist_entry(&pool, bob, "uuid-1")
        .await
        .unwrap()
        .is_none());
}

/// A wishlist-only book, minted the way a wishlist add mints one.
async fn seed_wished(pool: &SqlitePool, user: i64, title: &str) -> String {
    let uuid = create_fileless_book(
        pool,
        FilelessBook {
            title: title.into(),
            authors: vec!["Ada Lovelace".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    add_wishlist_entry(pool, user, &uuid, WishlistSource::Search)
        .await
        .unwrap();
    uuid
}

#[tokio::test]
async fn remove_wishlist_entry_purges_a_book_that_existed_only_to_be_wanted() {
    let _covers = CoversTempDir::new("wishlist_purge");
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let uuid = seed_wished(&pool, user, "Wanted").await;

    let removal = remove_wishlist_entry(&pool, user, &uuid).await.unwrap();

    assert!(removal.book_deleted);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 0);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM wishlist_entries").await,
        0
    );
}

#[tokio::test]
async fn remove_wishlist_entry_keeps_a_book_another_reader_still_wants() {
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let other = seed_user(&pool, "other").await;
    let uuid = seed_wished(&pool, user, "Wanted").await;
    add_wishlist_entry(&pool, other, &uuid, WishlistSource::Detail)
        .await
        .unwrap();

    let removal = remove_wishlist_entry(&pool, user, &uuid).await.unwrap();

    assert!(!removal.book_deleted);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert!(get_wishlist_entry(&pool, other, &uuid)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn remove_wishlist_entry_keeps_a_ghosted_library_book() {
    // No file, but a real scan root: the reindex owns that row and its
    // retention, and a reader unwishing it must not delete it out from under.
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;
    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    let removal = remove_wishlist_entry(&pool, user, "uuid-1").await.unwrap();

    assert!(!removal.book_deleted);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
}
