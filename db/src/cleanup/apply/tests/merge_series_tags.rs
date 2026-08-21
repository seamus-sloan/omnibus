//! `apply_merge_series` and `apply_merge_tags`: link migration across one
//! or several books, with undo restoring exactly the pre-merge link set.

use super::super::*;
use super::{
    alias_canonical, count_rows, fts_tags, insert_book, insert_series, insert_tag, link_series,
    link_tag, new_pool, seed_root, undo,
};

#[tokio::test]
async fn apply_merge_series_moves_link_and_undo_restores_state() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_series(&pool, "Foundation Series").await;
    let canonical = insert_series(&pool, "The Foundation Series").await;
    link_series(&pool, book, source).await;

    let log_id = apply_merge_series(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM series").await, 1);
    assert_eq!(
        count_rows(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM books_series_link WHERE book = {book} AND series = {canonical}"
            )
        )
        .await,
        1
    );
    assert!(alias_canonical(&pool, "series", "Foundation Series")
        .await
        .is_some());

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM series").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_series_link WHERE book = {book}")
        )
        .await,
        1,
        "the book must be linked to exactly the recreated source, not the former canonical"
    );
    assert_eq!(
        alias_canonical(&pool, "series", "Foundation Series").await,
        None
    );
}

#[tokio::test]
async fn apply_merge_tags_moves_link_across_three_books_and_undo_restores_state() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book_a = insert_book(&pool, lib, "book-a", "A").await;
    let book_b = insert_book(&pool, lib, "book-b", "B").await;
    let source = insert_tag(&pool, "scifi").await;
    let canonical = insert_tag(&pool, "sci-fi").await;
    link_tag(&pool, book_a, source).await;
    link_tag(&pool, book_b, canonical).await;
    link_tag(&pool, book_b, source).await;

    let log_id = apply_merge_tags(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
    assert!(fts_tags(&pool, book_a).await.contains("sci-fi"));
    assert!(fts_tags(&pool, book_b).await.contains("sci-fi"));

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book_a}")
        )
        .await,
        1
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book_b}")
        )
        .await,
        2,
        "book_b's independent pre-merge tag link must survive the round trip"
    );
}
