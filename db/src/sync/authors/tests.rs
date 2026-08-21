//! Unit tests for [`insert_author_links`]'s reindex-resurrection guard
//! (#964): an aliased name resolves straight to its canonical id without
//! minting a new `authors` row, while an unaliased name still goes through
//! the normal upsert-and-join path unchanged.

use std::collections::HashMap;

use omnibus_shared::{Contributor, EbookMetadata};

use super::*;
use crate::pool::init_db;

fn metadata_with_creator(name: &str) -> EbookMetadata {
    EbookMetadata {
        filename: "book.epub".into(),
        creators: vec![Contributor {
            name: name.into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

async fn seed_book(pool: &sqlx::SqlitePool) -> i64 {
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES ('u', ?, '/lib/u', 'T') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn author_id(pool: &sqlx::SqlitePool, name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn linked_author_ids(pool: &sqlx::SqlitePool, book_id: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT author FROM books_authors_link WHERE book = ? ORDER BY position")
        .bind(book_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn insert_author_links_upserts_and_links_an_unaliased_name() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book(&pool).await;
    let m = metadata_with_creator("Ada Lovelace");

    let mut tx = pool.begin().await.unwrap();
    insert_author_links(&mut tx, book_id, &m, &HashMap::new())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let id = author_id(&pool, "Ada Lovelace")
        .await
        .expect("author row must exist");
    assert_eq!(linked_author_ids(&pool, book_id).await, vec![id]);
}

#[tokio::test]
async fn insert_author_links_resolves_a_merged_away_name_to_its_canonical_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book(&pool).await;

    let canonical_id: i64 =
        sqlx::query_scalar("INSERT INTO authors (name) VALUES ('J. Smith') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id) \
         VALUES ('author', 'John Smith', ?)",
    )
    .bind(canonical_id)
    .execute(&pool)
    .await
    .unwrap();

    // Simulates a reindex of a file the parser still reports as "John
    // Smith" — the on-disk metadata a completed merge doesn't rewrite.
    // #1985: the alias map is now pre-resolved by the caller (here, built
    // directly from the row just inserted) rather than looked up inside
    // `insert_author_links` itself.
    let m = metadata_with_creator("John Smith");
    let author_aliases = HashMap::from([("John Smith".to_string(), canonical_id)]);
    let mut tx = pool.begin().await.unwrap();
    insert_author_links(&mut tx, book_id, &m, &author_aliases)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        author_id(&pool, "John Smith").await.is_none(),
        "AC2: the merged-away name must not be recreated"
    );
    assert_eq!(
        linked_author_ids(&pool, book_id).await,
        vec![canonical_id],
        "AC1: the book must link to the surviving canonical author"
    );
}

#[tokio::test]
async fn insert_author_links_skips_a_name_that_is_both_blocklisted_and_aliased() {
    // Precedence contract: the `ignored_authors` filter runs before alias
    // resolution, so a name carrying both a blocklist entry and an alias is
    // skipped outright. The blocklist recovery flow relies on this — a
    // convert-to-alias must *remove* the blocklist row, not merely add the
    // alias, or nothing changes at reindex time.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book(&pool).await;

    let canonical_id: i64 =
        sqlx::query_scalar("INSERT INTO authors (name) VALUES ('Andy Weir') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id) \
         VALUES ('author', 'Weir, Andy', ?)",
    )
    .bind(canonical_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO ignored_authors (name) VALUES ('Weir, Andy')")
        .execute(&pool)
        .await
        .unwrap();

    let m = metadata_with_creator("Weir, Andy");
    let author_aliases = HashMap::from([("Weir, Andy".to_string(), canonical_id)]);
    let mut tx = pool.begin().await.unwrap();
    insert_author_links(&mut tx, book_id, &m, &author_aliases)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        linked_author_ids(&pool, book_id).await,
        Vec::<i64>::new(),
        "the blocklist must win over the alias while both exist"
    );
}
