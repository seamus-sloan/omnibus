//! Tests for the `dedup_suggestions` review queue, split by sub-topic into
//! the sibling modules below; the suggestion, entity and reviewer seeding
//! fixtures they share live here.

mod decide;
mod internals;
mod queue;

use omnibus_shared::{CleanupAction, CleanupKind, Decision};
use sqlx::SqlitePool;

use crate::pool::init_db;

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

/// Insert one `dedup_suggestions` row and return its id.
async fn seed_suggestion(
    pool: &SqlitePool,
    kind: CleanupKind,
    action: CleanupAction,
    payload_json: &str,
    decision: Decision,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO dedup_suggestions (kind, action, payload_json, decision)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(kind.as_str())
    .bind(action.as_str())
    .bind(payload_json)
    .bind(decision.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

fn merge_payload(source_ids: &[i64], canonical_id: i64) -> String {
    let names: Vec<String> = source_ids.iter().map(|i| format!("Source {i}")).collect();
    serde_json::json!({
        "type": "merge",
        "source_ids": source_ids,
        "source_names": names,
        "canonical_id": canonical_id,
        "canonical_name": "Canonical Name",
    })
    .to_string()
}

/// Seed `count` authors plus one book linked to each, returning the author ids.
async fn seed_authors_with_books(pool: &SqlitePool, count: i64) -> Vec<i64> {
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for n in 0..count {
        let author_id: i64 =
            sqlx::query_scalar("INSERT INTO authors (name) VALUES (?) RETURNING id")
                .bind(format!("Author {n}"))
                .fetch_one(pool)
                .await
                .unwrap();
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books (uuid, scan_key, library_id, path, title)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(format!("uuid-{n}"))
        .bind(format!("book-{n}.epub"))
        .bind(lib_id)
        .bind(format!("/lib/book-{n}.epub"))
        .bind(format!("Book {n}"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO books_authors_link (book, author) VALUES (?, ?)")
            .bind(book_id)
            .bind(author_id)
            .execute(pool)
            .await
            .unwrap();
        ids.push(author_id);
    }
    ids
}

/// `dedup_suggestions.decided_by` carries a real FK to `users`, so a decide
/// test needs an actual reviewer row.
async fn seed_reviewer(pool: &SqlitePool) -> i64 {
    crate::auth::create_user(pool, "reviewer", "correct horse battery staple")
        .await
        .unwrap();
    crate::auth::get_user_by_username(pool, "reviewer")
        .await
        .unwrap()
        .unwrap()
        .id
}
