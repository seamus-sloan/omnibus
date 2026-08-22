//! Physical pseudo-root promotion: the per-book attach-time move and the
//! boot backfill, including the cases that must leave a book where it is.

use sqlx::SqlitePool;

use crate::test_support::seed_minimal_books;

use super::super::*;
use super::pool;

/// Fileless book with no cover — the minimal promotion fixture.
async fn seed_wishlist_book(pool: &SqlitePool, title: &str, author: &str) -> (i64, String) {
    let uuid = create_fileless_book(
        pool,
        FilelessBook {
            title: title.into(),
            authors: vec![author.into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    let id = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    (id, uuid)
}

/// Insert a real scan root and return its id.
async fn seed_real_root(pool: &SqlitePool, path: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib') RETURNING id")
        .bind(path)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Forge the stranded shape the pre-guard attach left behind: a file row
/// rooted at `library_path` hanging off a book still under the pseudo-root.
async fn plant_attached_file(pool: &SqlitePool, book_id: i64, library_path: &str, scan_key: &str) {
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, library_path, path)
         VALUES (?, 'EPUB', ?, 1, 1, ?, ?, '')",
    )
    .bind(book_id)
    .bind(scan_key)
    .bind(scan_key)
    .bind(library_path)
    .execute(pool)
    .await
    .unwrap();
}

async fn library_path_of(pool: &SqlitePool, book_id: i64) -> String {
    sqlx::query_scalar(
        "SELECT l.path FROM books b JOIN scan_roots l ON l.id = b.library_id WHERE b.id = ?",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn promote_filed_physical_book_moves_wishlist_book_into_the_files_library() {
    let pool = pool().await;
    let (book_id, uuid) = seed_wishlist_book(&pool, "Dream by the Shadows", "Karlie Logan").await;
    seed_real_root(&pool, "/lib").await;
    plant_attached_file(&pool, book_id, "/lib", "Logan/dream.epub").await;

    let mut conn = pool.acquire().await.unwrap();
    let promoted = promote_filed_physical_book(&mut conn, book_id)
        .await
        .unwrap();
    drop(conn);

    assert!(promoted);
    assert_eq!(library_path_of(&pool, book_id).await, "/lib");
    // The point of the promotion: both path-scoped read surfaces now see it.
    let listed = crate::books::list_books_for_paths(&pool, &["/lib"])
        .await
        .unwrap();
    assert!(listed
        .iter()
        .any(|b| b.unique_identifier.as_deref() == Some(uuid.as_str())));
    let found = crate::books::search_books(&pool, "/lib", "Shadows")
        .await
        .unwrap();
    assert!(found
        .iter()
        .any(|b| b.unique_identifier.as_deref() == Some(uuid.as_str())));
}

#[tokio::test]
async fn promote_filed_physical_book_keeps_a_real_library_book_in_place() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let promoted = promote_filed_physical_book(&mut conn, book_id)
        .await
        .unwrap();

    assert!(!promoted);
    assert_eq!(library_path_of(&pool, book_id).await, "/lib");
}

#[tokio::test]
async fn promote_filed_physical_book_leaves_a_still_fileless_book_hidden() {
    let pool = pool().await;
    let (book_id, uuid) = seed_wishlist_book(&pool, "Paper Only", "Jane Doe").await;
    seed_real_root(&pool, "/lib").await;

    let mut conn = pool.acquire().await.unwrap();
    let promoted = promote_filed_physical_book(&mut conn, book_id)
        .await
        .unwrap();
    drop(conn);

    assert!(!promoted);
    let listed = crate::books::list_books_for_paths(&pool, &["/lib"])
        .await
        .unwrap();
    assert!(!listed
        .iter()
        .any(|b| b.unique_identifier.as_deref() == Some(uuid.as_str())));
}

#[tokio::test]
async fn promote_filed_physical_book_ignores_files_rooted_at_the_physical_root() {
    let pool = pool().await;
    let (book_id, _uuid) = seed_wishlist_book(&pool, "Paper Only", "Jane Doe").await;
    // A fileless-into-fileless merge leaves a file "rooted" at the pseudo-root
    // itself — no real root to promote into, so the book must stay put.
    plant_attached_file(&pool, book_id, "physical://local", "x.epub").await;

    let mut conn = pool.acquire().await.unwrap();
    let promoted = promote_filed_physical_book(&mut conn, book_id)
        .await
        .unwrap();

    assert!(!promoted);
    assert_eq!(library_path_of(&pool, book_id).await, "physical://local");
}

#[tokio::test]
async fn promote_filed_physical_books_backfills_stranded_rows_and_is_idempotent() {
    let pool = pool().await;
    seed_real_root(&pool, "/lib").await;
    let (stranded_a, _) = seed_wishlist_book(&pool, "Stranded Alpha", "Ann Author").await;
    let (stranded_b, _) = seed_wishlist_book(&pool, "Stranded Beta", "Bob Author").await;
    let (fileless, _) = seed_wishlist_book(&pool, "Genuinely Fileless", "Cay Author").await;
    plant_attached_file(&pool, stranded_a, "/lib", "a/a.epub").await;
    plant_attached_file(&pool, stranded_b, "/lib", "b/b.epub").await;

    assert_eq!(promote_filed_physical_books(&pool).await.unwrap(), 2);
    assert_eq!(library_path_of(&pool, stranded_a).await, "/lib");
    assert_eq!(library_path_of(&pool, stranded_b).await, "/lib");
    assert_eq!(library_path_of(&pool, fileless).await, "physical://local");

    // Caught up: the second run touches nothing.
    assert_eq!(promote_filed_physical_books(&pool).await.unwrap(), 0);
}
