//! Unit tests for [`resolve_entity_aliases`]: empty input, no match, a
//! single hit, a mixed batch, and kind-scoping (an alias recorded under one
//! `CleanupKind` must never leak into a lookup for another).

use omnibus_shared::CleanupKind;

use super::*;
use crate::pool::init_db;

async fn insert_alias(
    pool: &sqlx::SqlitePool,
    kind: CleanupKind,
    alias_name: &str,
    canonical_id: i64,
) {
    sqlx::query("INSERT INTO entity_aliases (kind, alias_name, canonical_id) VALUES (?, ?, ?)")
        .bind(kind.as_str())
        .bind(alias_name)
        .bind(canonical_id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn resolve_entity_aliases_returns_empty_map_for_empty_names() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let found = resolve_entity_aliases(&mut tx, CleanupKind::Author, &[])
        .await
        .unwrap();
    assert!(found.is_empty());
}

#[tokio::test]
async fn resolve_entity_aliases_returns_empty_map_when_no_alias_recorded() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let found = resolve_entity_aliases(&mut tx, CleanupKind::Author, &["Nobody"])
        .await
        .unwrap();
    assert!(found.is_empty());
}

#[tokio::test]
async fn resolve_entity_aliases_finds_a_recorded_alias() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    insert_alias(&pool, CleanupKind::Author, "John Smith", 42).await;

    let mut tx = pool.begin().await.unwrap();
    let found = resolve_entity_aliases(&mut tx, CleanupKind::Author, &["John Smith"])
        .await
        .unwrap();
    assert_eq!(found.get("John Smith"), Some(&42));
}

#[tokio::test]
async fn resolve_entity_aliases_returns_only_the_names_that_hit_in_a_mixed_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    insert_alias(&pool, CleanupKind::Series, "The Wheel of Time", 7).await;

    let mut tx = pool.begin().await.unwrap();
    let found = resolve_entity_aliases(
        &mut tx,
        CleanupKind::Series,
        &["The Wheel of Time", "Unrelated Series"],
    )
    .await
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found.get("The Wheel of Time"), Some(&7));
    assert!(!found.contains_key("Unrelated Series"));
}

#[tokio::test]
async fn resolve_entity_aliases_does_not_leak_across_kinds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    insert_alias(&pool, CleanupKind::Author, "Ambiguous Name", 1).await;

    let mut tx = pool.begin().await.unwrap();
    let found = resolve_entity_aliases(&mut tx, CleanupKind::Series, &["Ambiguous Name"])
        .await
        .unwrap();
    assert!(
        found.is_empty(),
        "an author-kind alias must not answer a series-kind lookup"
    );
}
