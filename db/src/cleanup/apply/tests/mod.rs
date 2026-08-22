//! Round-trip coverage for the apply/undo primitives: one happy-path
//! apply-then-undo test per primitive, plus the error variants each can
//! return. Split by primitive into the sibling modules below; shared seed
//! helpers live here. All tests run against `sqlite::memory:` per
//! [`crate::pool::init_db`].

mod book_title_override;
mod delete_author;
mod merge_authors;
mod merge_series_tags;
mod tag_split;
mod undo_errors;

use sqlx::SqlitePool;

use super::super::undo::undo;
use super::*;
use crate::pool::init_db;

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

async fn seed_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ('admin', 'x', 1) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_book(pool: &SqlitePool, lib_id: i64, uuid: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(format!("/lib/{uuid}"))
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_author(pool: &SqlitePool, name: &str, sort: Option<&str>) -> i64 {
    sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(sort)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_series(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO series (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_tag(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO tags (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn link_author(pool: &SqlitePool, book_id: i64, author_id: i64, position: i64) {
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, ?)")
        .bind(book_id)
        .bind(author_id)
        .bind(position)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_series(pool: &SqlitePool, book_id: i64, series_id: i64) {
    sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?, ?)")
        .bind(book_id)
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_tag(pool: &SqlitePool, book_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(book_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_author_photo(
    pool: &SqlitePool,
    author_id: i64,
    source: &str,
    bytes: Option<&[u8]>,
) {
    sqlx::query("INSERT INTO author_photos (author_id, source, mime, bytes) VALUES (?, ?, ?, ?)")
        .bind(author_id)
        .bind(source)
        .bind(bytes.map(|_| "image/png"))
        .bind(bytes)
        .execute(pool)
        .await
        .unwrap();
}

async fn author_photo_source(pool: &SqlitePool, author_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT source FROM author_photos WHERE author_id = ?")
        .bind(author_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn author_row(pool: &SqlitePool, name: &str) -> Option<(i64, Option<String>)> {
    sqlx::query_as("SELECT id, sort FROM authors WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn author_position(pool: &SqlitePool, book_id: i64, author_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT position FROM books_authors_link WHERE book = ? AND author = ?")
        .bind(book_id)
        .bind(author_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn count_rows(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn fts_authors(pool: &SqlitePool, book_id: i64) -> String {
    sqlx::query_scalar("SELECT authors FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn fts_tags(pool: &SqlitePool, book_id: i64) -> String {
    sqlx::query_scalar("SELECT tags FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn is_ignored_author(pool: &SqlitePool, name: &str) -> bool {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    count > 0
}

async fn alias_canonical(pool: &SqlitePool, kind: &str, alias_name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT canonical_id FROM entity_aliases WHERE kind = ? AND alias_name = ?")
        .bind(kind)
        .bind(alias_name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// Split `results` into (ok_count, already_undone_count) for a two-caller
/// race, asserting nothing else showed up (a panic, a DB error, etc. would
/// mean the race produced more than the two expected outcomes). Shared by
/// the merge and rename concurrent-undo tests.
fn tally_race_outcomes(results: &[Result<(), CleanupApplyError>]) -> (usize, usize) {
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let already_undone = results
        .iter()
        .filter(|r| matches!(r, Err(CleanupApplyError::AlreadyUndone)))
        .count();
    assert_eq!(
        ok + already_undone,
        results.len(),
        "every racing call must resolve to either success or AlreadyUndone, nothing else"
    );
    (ok, already_undone)
}
