//! `apply_delete_author`: blocklisting the name so a future rescan can't
//! recreate it, and the not-found refusal for an unknown id.

use super::super::*;
use super::{
    count_rows, fts_authors, insert_author, insert_book, is_ignored_author, link_author, new_pool,
    seed_root, undo,
};

#[tokio::test]
async fn apply_delete_author_blocklists_name_and_undo_restores_author() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let author = insert_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]", None).await;
    link_author(&pool, book, author, 0).await;

    let log_id = apply_delete_author(&pool, author, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM authors").await, 0);
    assert!(is_ignored_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]").await);
    assert!(!fts_authors(&pool, book).await.contains("calibre"));

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM authors").await, 1);
    assert!(!is_ignored_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]").await);
    assert!(fts_authors(&pool, book).await.contains("calibre"));
}

#[tokio::test]
async fn apply_delete_author_returns_not_found_for_unknown_id() {
    let pool = new_pool().await;
    let err = apply_delete_author(&pool, 9999, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::NotFound(9999)));
}
