//! Fileless book removal: the uuid-keyed soft-reference sweep, and the two
//! refusals (`BookHasFiles`, `BookNotFound`).

use omnibus_shared::physical::WishlistSource;

use crate::test_support::{seed_minimal_books, CoversTempDir};

use super::super::*;
use super::{count, pool, seed_user};

#[tokio::test]
async fn delete_fileless_book_removes_the_book_and_its_soft_refs() {
    let _covers = CoversTempDir::new("delete_fileless");
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Paper Only".into(),
            authors: vec!["Ada Lovelace".into()],
            isbn: Some("9780000000001".into()),
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    add_physical_copy(&pool, &uuid, None, Some(user), None)
        .await
        .unwrap();
    add_wishlist_entry(&pool, user, &uuid, WishlistSource::Detail)
        .await
        .unwrap();
    // A uuid-keyed row *outside* the physical/wishlist/override trio, to prove
    // the delete sweeps the full canonical set rather than a hand-picked few.
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?1, ?2, 8)")
        .bind(user)
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    delete_fileless_book(&pool, &uuid).await.unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 0);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM physical_copies").await,
        0
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM wishlist_entries").await,
        0
    );
    // The wider uuid-keyed sweep: the rating must go too.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM user_ratings").await, 0);
    // The FTS twin has no FK, so it only goes when the book is hard-deleted.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books_fts").await, 0);
    // Its now-bookless author goes with it (orphan taxonomy sweep).
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM authors").await, 0);
}

#[tokio::test]
async fn delete_fileless_book_refuses_a_book_that_still_has_files() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;

    let err = delete_fileless_book(&pool, "uuid-1").await.unwrap_err();

    assert!(matches!(err, PhysicalError::BookHasFiles));
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

#[tokio::test]
async fn delete_fileless_book_returns_book_not_found_for_unknown_uuid() {
    let pool = pool().await;
    let err = delete_fileless_book(&pool, "nope").await.unwrap_err();
    assert!(matches!(err, PhysicalError::BookNotFound));
}
