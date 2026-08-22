//! ISBN handling: surfacing an ISBN from `book_identifiers` after the column
//! drop, deriving `isbn13` from a scanned ISBN-scheme identifier, and the
//! override that wins over (or clears) the scanned value.

use omnibus_shared::{EbookMetadata, Identifier, MetadataOverrides};

use crate::ebook::IndexedBook;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::CoversTempDir;

use super::super::*;

/// F8 regression: the denormalized `books.isbn` column was dropped (migration
/// 0023). A book's ISBN must still surface through the identifier projection,
/// which reads the canonical `book_identifiers` rows — proving the read path
/// never depended on the removed column.
#[tokio::test]
async fn get_book_and_list_books_surface_isbn_from_book_identifiers_after_column_drop() {
    let _covers = CoversTempDir::new("isbn_after_drop");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "isbn.epub".into(),
                title: Some("ISBN Book".into()),
                identifiers: vec![Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("isbn".into()),
                }],
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();

    // The `books` table no longer has an `isbn` column at all.
    let has_isbn_col: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('books') WHERE name = 'isbn'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(has_isbn_col, 0, "books.isbn must be dropped");

    let list = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(list.len(), 1);
    let list_isbn = list[0]
        .identifiers
        .iter()
        .find(|i| i.scheme.as_deref() == Some("isbn"))
        .map(|i| i.value.as_str());
    assert_eq!(list_isbn, Some("9780000000000"));

    let detail = get_book(&pool, list[0].id).await.unwrap().unwrap();
    let detail_isbn = detail
        .identifiers
        .iter()
        .find(|i| i.scheme.as_deref() == Some("isbn"))
        .map(|i| i.value.as_str());
    assert_eq!(detail_isbn, Some("9780000000000"));
}

// ---------- ISBN-13 (issue #1088) ----------

/// Seed one book with a single scanned identifier and return the row's
/// `books.id` alongside its stable uuid.
async fn seed_book_with_identifier(
    pool: &sqlx::SqlitePool,
    scheme: &str,
    value: &str,
) -> (i64, String) {
    replace_books(
        pool,
        "/lib",
        vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "isbn.epub".into(),
                title: Some("Book With Identifier".into()),
                identifiers: vec![Identifier {
                    value: value.into(),
                    scheme: Some(scheme.into()),
                }],
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();
    let books = list_books(pool, "/lib").await.unwrap();
    let book = &books[0];
    (book.id, book.unique_identifier.clone().unwrap())
}

#[tokio::test]
async fn get_book_derives_isbn13_from_scanned_isbn_scheme_identifier() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "ISBN", "9780134685991").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780134685991"));
}

#[tokio::test]
async fn get_book_strips_hyphens_when_deriving_isbn13() {
    // OPF ISBN values are commonly hyphenated; the derivation strips
    // non-digit characters before checking the 13-digit length.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "ISBN", "978-0-13-468599-1").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780134685991"));
}

#[tokio::test]
async fn get_book_ignores_ten_digit_isbn_when_deriving_isbn13() {
    // An ISBN-10 (10 digits) is a distinct identifier value; it must not be
    // mistaken for an ISBN-13.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "ISBN", "0134685997").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert!(book.isbn13.is_none());
}

#[tokio::test]
async fn get_book_isbn13_is_none_when_no_isbn_scheme_identifier_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "calibre", "some-opaque-id").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert!(book.isbn13.is_none());
}

#[tokio::test]
async fn get_book_isbn13_override_wins_over_scanned_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (id, uuid) = seed_book_with_identifier(&pool, "ISBN", "9780134685991").await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            isbn13: Some("9780316769488".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780316769488"));
}

#[tokio::test]
async fn get_book_isbn13_override_clears_scanned_value_with_empty_string() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (id, uuid) = seed_book_with_identifier(&pool, "ISBN", "9780134685991").await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            isbn13: Some(String::new()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert!(
        book.isbn13.is_none(),
        "an empty-string override must clear the scanned ISBN-13, not persist as an empty string"
    );
}

#[tokio::test]
async fn get_book_isbn13_override_applies_when_no_scanned_value_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (id, uuid) = seed_book_with_identifier(&pool, "calibre", "opaque-id").await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            isbn13: Some("9780134685991".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780134685991"));
}
