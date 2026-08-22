//! Physical copies: check-in (multiple per book, merged-uuid resolution),
//! individual deletion, note editing, and the check-in's cross-user
//! wishlist fulfillment.

use omnibus_shared::physical::WishlistSource;

use crate::test_support::seed_minimal_books;

use super::super::*;
use super::{pool, seed_user};

#[tokio::test]
async fn add_physical_copy_returns_the_checked_in_row() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;

    let copy = add_physical_copy(&pool, "uuid-1", Some("9780000000001"), None, Some("1st ed"))
        .await
        .unwrap();

    assert_eq!(copy.book_uuid, "uuid-1");
    assert_eq!(copy.isbn.as_deref(), Some("9780000000001"));
    assert_eq!(copy.note.as_deref(), Some("1st ed"));
    assert!(copy.checked_in_at > 0);
}

#[tokio::test]
async fn add_physical_copy_allows_multiple_copies_per_book() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;

    add_physical_copy(&pool, "uuid-1", Some("a"), None, None)
        .await
        .unwrap();
    add_physical_copy(&pool, "uuid-1", Some("b"), None, None)
        .await
        .unwrap();

    let copies = list_physical_copies(&pool, "uuid-1").await.unwrap();
    assert_eq!(copies.len(), 2);
}

#[tokio::test]
async fn add_physical_copy_errors_when_book_missing() {
    let pool = pool().await;
    let err = add_physical_copy(&pool, "nope", None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, PhysicalError::BookNotFound));
}

#[tokio::test]
async fn add_physical_copy_resolves_a_merged_uuid() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // A uuid that was merged into uuid-1 must still resolve to the live book.
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('old-uuid', ?1, 'EPUB', '/lib')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    // Checked in against the merged ledger key, the copy is stored under the
    // canonical (surviving) uuid — the value the rest of the system keys on.
    let copy = add_physical_copy(&pool, "old-uuid", None, None, None)
        .await
        .unwrap();
    assert_eq!(copy.book_uuid, "uuid-1");
    // And it's findable by either uuid, since the read path folds to canonical.
    assert_eq!(
        list_physical_copies(&pool, "old-uuid").await.unwrap().len(),
        1
    );
    assert_eq!(
        list_physical_copies(&pool, "uuid-1").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn delete_physical_copy_removes_one_copy() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let keep = add_physical_copy(&pool, "uuid-1", None, None, None)
        .await
        .unwrap();
    let drop = add_physical_copy(&pool, "uuid-1", None, None, None)
        .await
        .unwrap();

    delete_physical_copy(&pool, drop.id).await.unwrap();

    let copies = list_physical_copies(&pool, "uuid-1").await.unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].id, keep.id);
}

#[tokio::test]
async fn delete_physical_copy_errors_when_missing() {
    let pool = pool().await;
    let err = delete_physical_copy(&pool, 999).await.unwrap_err();
    assert!(matches!(err, PhysicalError::CopyNotFound));
}

// --- fulfillment (AC3) -----------------------------------------------------

#[tokio::test]
async fn add_physical_copy_fulfills_every_users_wishlist() {
    let pool = pool().await;
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();
    add_wishlist_entry(&pool, bob, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();
    // An unrelated wishlist entry must survive.
    add_wishlist_entry(&pool, alice, "uuid-2", WishlistSource::Scan)
        .await
        .unwrap();

    add_physical_copy(&pool, "uuid-1", None, None, None)
        .await
        .unwrap();

    assert!(list_wishlist(&pool, bob).await.unwrap().is_empty());
    let alice_left = list_wishlist(&pool, alice).await.unwrap();
    assert_eq!(alice_left.len(), 1);
    assert_eq!(alice_left[0].book_uuid, "uuid-2");
}

// --- note editing ----------------------------------------------------------

#[tokio::test]
async fn update_physical_copy_note_replaces_the_note() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let copy = add_physical_copy(&pool, "uuid-1", None, None, Some("1st ed"))
        .await
        .unwrap();

    let updated = update_physical_copy_note(&pool, copy.id, Some("signed 1st ed"))
        .await
        .unwrap();

    assert_eq!(updated.id, copy.id);
    assert_eq!(updated.note.as_deref(), Some("signed 1st ed"));
}

#[tokio::test]
async fn update_physical_copy_note_clears_the_note_when_blank() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let copy = add_physical_copy(&pool, "uuid-1", None, None, Some("1st ed"))
        .await
        .unwrap();

    // A whitespace-only note from an emptied input is a clear, not a value.
    let updated = update_physical_copy_note(&pool, copy.id, Some("   "))
        .await
        .unwrap();

    assert_eq!(updated.note, None);
}

#[tokio::test]
async fn update_physical_copy_note_returns_copy_not_found_for_unknown_id() {
    let pool = pool().await;
    let err = update_physical_copy_note(&pool, 9999, Some("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, PhysicalError::CopyNotFound));
}
