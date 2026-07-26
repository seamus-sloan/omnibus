use super::*;
use crate::init_db;
use crate::test_support::seed_synced_ebook;

#[tokio::test]
async fn sync_books_returns_seeded_books_with_uuid_and_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let rows = sync_books(&pool).await.unwrap();

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.uuid, uuid);
    assert_eq!(row.title, "Dune");
    assert_eq!(row.author, "Frank Herbert");
}

#[tokio::test]
async fn sync_books_returns_empty_for_empty_library() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let rows = sync_books(&pool).await.unwrap();

    assert!(rows.is_empty());
}

#[tokio::test]
async fn sync_books_enumerates_every_book_with_no_cap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    for i in 0..150 {
        seed_synced_ebook(
            &pool,
            &format!("book-{i}.epub"),
            &format!("Title {i}"),
            "Author",
        )
        .await;
    }

    let rows = sync_books(&pool).await.unwrap();

    // Deliberately never adopts Calibre-Web's SYNC_ITEM_LIMIT=100 cap.
    assert_eq!(rows.len(), 150);
}
