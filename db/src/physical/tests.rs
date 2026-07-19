//! Physical ownership data-layer tests: copies (multiple per book,
//! individually deletable), wishlist (per-user, idempotent), check-in
//! fulfillment across all users, and fileless book creation.

use sqlx::SqlitePool;

use omnibus_shared::physical::WishlistSource;

use super::*;
use crate::covers::cover_path_for;
use crate::test_support::{seed_minimal_books, CoversTempDir};

async fn pool() -> SqlitePool {
    crate::pool::init_db("sqlite::memory:").await.unwrap()
}

async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    // Insert directly: `create_user` gates all but the first registration on the
    // registration-enabled setting, and these tests only need user rows for FKs.
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash) VALUES (?1, 'x') RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .unwrap()
}

// --- physical copies -------------------------------------------------------

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

    let copy = add_physical_copy(&pool, "old-uuid", None, None, None)
        .await
        .unwrap();
    assert_eq!(copy.book_uuid, "old-uuid");
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

// --- wishlist --------------------------------------------------------------

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

// --- fileless book (AC4) ---------------------------------------------------

#[tokio::test]
async fn create_fileless_book_makes_a_uuid_row_with_identifier_and_no_files() {
    let _covers = CoversTempDir::new("fileless");
    let pool = pool().await;

    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Physical Only".into(),
            authors: vec!["Jane Doe".into()],
            isbn: Some("9781111111111".into()),
            pubdate: Some("2021".into()),
            description: Some("A print book".into()),
            cover: Some(FilelessCover {
                mime: "image/gif".into(),
                bytes: GIF_1X1.to_vec(),
            }),
        },
    )
    .await
    .unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    // A uuid'd books row with the ISBN identifier, cover flagged, and no files.
    let has_cover: i64 = sqlx::query_scalar("SELECT has_cover FROM books WHERE id = ?1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(has_cover, 1);
    assert!(cover_path_for(&uuid, "gif").exists());

    let isbn: String = sqlx::query_scalar(
        "SELECT value FROM book_identifiers WHERE book_id = ?1 AND scheme = 'ISBN'",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(isbn, "9781111111111");

    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM book_files WHERE book_id = {book_id}")
        )
        .await,
        0
    );
    // Author linked.
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1
    );
}

#[tokio::test]
async fn create_fileless_book_reuses_one_physical_scan_root() {
    let _covers = CoversTempDir::new("fileless_root");
    let pool = pool().await;

    for title in ["One", "Two"] {
        create_fileless_book(
            &pool,
            FilelessBook {
                title: title.into(),
                authors: vec![],
                isbn: None,
                pubdate: None,
                description: None,
                cover: None,
            },
        )
        .await
        .unwrap();
    }

    // Both fileless books share the single synthetic Physical scan root.
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM scan_roots WHERE path = 'physical://local'"
        )
        .await,
        1
    );
    let physical_books = count(
        &pool,
        "SELECT COUNT(*) FROM books b JOIN scan_roots s ON b.library_id = s.id
         WHERE s.path = 'physical://local'",
    )
    .await;
    assert_eq!(physical_books, 2);
}

/// Minimal valid 1x1 GIF87a — enough for `image` to sniff and write `<uuid>.gif`.
const GIF_1X1: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x37, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xFF, 0xFF, 0xFF, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44,
    0x01, 0x00, 0x3B,
];
