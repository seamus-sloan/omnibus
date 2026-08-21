//! Coverage for the `ignored_authors` blocklist management surface: listing,
//! the convert-to-alias primitive (happy + per-variant errors + undo
//! round-trip), and the plain remove. All tests run against
//! `sqlite::memory:` per [`crate::pool::init_db`].

use sqlx::SqlitePool;

use super::super::apply::CleanupApplyError;
use super::super::undo::undo;
use super::*;
use crate::pool::init_db;

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

async fn insert_author(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO authors (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_ignored(pool: &SqlitePool, name: &str, ignored_at: i64) {
    sqlx::query("INSERT INTO ignored_authors (name, ignored_at) VALUES (?, ?)")
        .bind(name)
        .bind(ignored_at)
        .execute(pool)
        .await
        .unwrap();
}

async fn fetch_alias(pool: &SqlitePool, name: &str) -> Option<(i64, i64)> {
    sqlx::query_as(
        "SELECT canonical_id, created_at FROM entity_aliases
          WHERE kind = 'author' AND alias_name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn fetch_ignored(pool: &SqlitePool, name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT ignored_at FROM ignored_authors WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn list_ignored_authors_returns_entries_alphabetically() {
    let pool = new_pool().await;
    insert_ignored(&pool, "Weir, Andy", 200).await;
    insert_ignored(&pool, "Smashwords, Inc.", 100).await;

    let entries = list_ignored_authors(&pool).await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Smashwords, Inc.", "Weir, Andy"]);
    assert_eq!(entries[0].ignored_at, 100);
}

#[tokio::test]
async fn apply_alias_ignored_author_moves_entry_into_entity_aliases_and_logs() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Andy Weir").await;
    insert_ignored(&pool, "Weir, Andy", 123).await;

    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ('admin', 'x', 1) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let log_id = apply_alias_ignored_author(&pool, "Weir, Andy", canonical, Some(admin))
        .await
        .unwrap();
    assert!(log_id > 0);

    assert_eq!(fetch_ignored(&pool, "Weir, Andy").await, None);
    let (alias_canonical, _) = fetch_alias(&pool, "Weir, Andy").await.unwrap();
    assert_eq!(alias_canonical, canonical);
}

#[tokio::test]
async fn apply_alias_ignored_author_rejects_name_not_on_the_blocklist() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Andy Weir").await;

    let err = apply_alias_ignored_author(&pool, "Weir, Andy", canonical, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::InvalidRequest(_)));
}

#[tokio::test]
async fn apply_alias_ignored_author_rejects_unknown_canonical_author() {
    let pool = new_pool().await;
    insert_ignored(&pool, "Weir, Andy", 123).await;

    let err = apply_alias_ignored_author(&pool, "Weir, Andy", 999, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::NotFound(999)));
}

#[tokio::test]
async fn undo_alias_ignored_author_restores_the_blocklist_row_and_removes_the_alias() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Andy Weir").await;
    insert_ignored(&pool, "Weir, Andy", 123).await;

    let log_id = apply_alias_ignored_author(&pool, "Weir, Andy", canonical, None)
        .await
        .unwrap();
    undo(&pool, log_id).await.unwrap();

    assert_eq!(fetch_ignored(&pool, "Weir, Andy").await, Some(123));
    assert_eq!(fetch_alias(&pool, "Weir, Andy").await, None);
}

#[tokio::test]
async fn undo_alias_ignored_author_restores_a_pre_existing_alias_mapping() {
    let pool = new_pool().await;
    let earlier = insert_author(&pool, "A. Weir").await;
    let canonical = insert_author(&pool, "Andy Weir").await;
    sqlx::query(
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id, created_at)
         VALUES ('author', 'Weir, Andy', ?, 55)",
    )
    .bind(earlier)
    .execute(&pool)
    .await
    .unwrap();
    insert_ignored(&pool, "Weir, Andy", 123).await;

    let log_id = apply_alias_ignored_author(&pool, "Weir, Andy", canonical, None)
        .await
        .unwrap();
    assert_eq!(
        fetch_alias(&pool, "Weir, Andy").await,
        Some((canonical, fetch_alias(&pool, "Weir, Andy").await.unwrap().1))
    );

    undo(&pool, log_id).await.unwrap();
    assert_eq!(fetch_alias(&pool, "Weir, Andy").await, Some((earlier, 55)));
    assert_eq!(fetch_ignored(&pool, "Weir, Andy").await, Some(123));
}

#[tokio::test]
async fn remove_ignored_author_deletes_the_entry() {
    let pool = new_pool().await;
    insert_ignored(&pool, "Smashwords, Inc.", 100).await;

    remove_ignored_author(&pool, "Smashwords, Inc.")
        .await
        .unwrap();
    assert_eq!(fetch_ignored(&pool, "Smashwords, Inc.").await, None);
}

#[tokio::test]
async fn remove_ignored_author_rejects_name_not_on_the_blocklist() {
    let pool = new_pool().await;
    let err = remove_ignored_author(&pool, "Nobody").await.unwrap_err();
    assert!(matches!(err, CleanupApplyError::InvalidRequest(_)));
}
