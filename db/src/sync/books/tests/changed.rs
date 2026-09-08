//! The Changed bucket: `sync_changed` updates the existing row in place,
//! is a no-op on an empty batch, and refreshes the persisted word and page
//! counts.

use super::super::shared::insert_book_row;
use super::super::{sync_changed, EntityAliasMaps};
use super::{seed_book_with_file, seed_scan_root};
use crate::pool::init_db;
use crate::test_support::{count_rows, indexed, CoversTempDir};

/// `sync_changed` updates an existing book in place — the `books.id` and
/// `books.uuid` are preserved while the scalar columns and link rows are
/// rewritten from the fresh parse.
#[tokio::test]
async fn sync_changed_updates_an_existing_book_row_in_place() {
    let _covers = CoversTempDir::new("sync_changed_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Same filename (→ same scan_key), new title + author.
    let changed = vec![indexed(
        "a.epub",
        Some("Updated Title"),
        &["New Author"],
        &[],
        None,
        None,
    )];
    let mut tx = pool.begin().await.unwrap();
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &changed,
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Exactly one row, same id + uuid, refreshed scalars.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "changed updates in place, not insert"
    );
    let (id_after, uuid_after, title_after): (i64, String, String) =
        sqlx::query_as("SELECT id, uuid, title FROM books WHERE scan_key = 'a.epub'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(id_after, book_id, "books.id preserved across change");
    assert_eq!(
        uuid_after, uuid_before,
        "books.uuid preserved across change"
    );
    assert_eq!(title_after, "Updated Title", "scalar columns refreshed");

    // The author link was wiped-and-rewritten to the new author only.
    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM authors a
         JOIN books_authors_link l ON l.author = a.id
         WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(authors, vec!["New Author".to_string()], "links rewritten");
}

/// `sync_changed` with an empty batch is a no-op and returns no cover
/// triples (early return before the id pre-fetch).
#[tokio::test]
async fn sync_changed_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(covers.is_empty());
}

/// `word_count` round-trips through both the insert and the in-place update:
/// `insert_book_row` persists the estimate from the `IndexedBook`, and
/// `sync_changed` refreshes it on re-parse (a book edited to a longer text).
#[tokio::test]
async fn word_count_persists_on_insert_and_refreshes_on_change() {
    let _covers = CoversTempDir::new("sync_word_count_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let mut b = indexed("wc.epub", Some("Counted"), &[], &[], None, None);
    b.word_count = Some(1000);
    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let stored: Option<i64> = sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, Some(1000), "insert persists the estimate");

    // Same filename (→ same scan_key), new word count — the Changed path.
    let mut changed = indexed("wc.epub", Some("Counted"), &[], &[], None, None);
    changed.word_count = Some(2500);
    let mut tx = pool.begin().await.unwrap();
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[changed],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let refreshed: Option<i64> = sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(refreshed, Some(2500), "the update refreshes word_count");
}

/// `page_count` round-trips the same way `word_count` does (#1593):
/// `insert_book_row` persists the CBZ parser's page count, and
/// `sync_changed` refreshes it on re-parse (a book edited to add pages).
#[tokio::test]
async fn page_count_persists_on_insert_and_refreshes_on_change() {
    let _covers = CoversTempDir::new("sync_page_count_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let mut b = indexed("pc.cbz", Some("Paged"), &[], &[], None, None);
    b.metadata.page_count = Some(12);
    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let stored: Option<i64> = sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, Some(12), "insert persists the page count");

    // Same filename (→ same scan_key), new page count — the Changed path.
    let mut changed = indexed("pc.cbz", Some("Paged"), &[], &[], None, None);
    changed.metadata.page_count = Some(20);
    let mut tx = pool.begin().await.unwrap();
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[changed],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let refreshed: Option<i64> = sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(refreshed, Some(20), "the update refreshes page_count");
}
