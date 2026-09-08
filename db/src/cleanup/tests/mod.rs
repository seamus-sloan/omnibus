//! Tests for the library-cleanup detectors, split by sub-topic into the
//! sibling modules below; the pool, seed and link fixtures they share live
//! here. Pure coverage of the detector building blocks, pool-backed
//! coverage of each `detect_*` entry point, and override-aware detection.

mod detectors;
mod overrides;
mod pure;
mod title_cruft;

use sqlx::SqlitePool;

use super::*;
use crate::pool::init_db;

// Seed helpers
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

async fn insert_series(pool: &SqlitePool, name: &str, sort: Option<&str>) -> i64 {
    sqlx::query_scalar("INSERT INTO series (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(sort)
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

async fn link_author(pool: &SqlitePool, book_id: i64, author_id: i64) {
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book_id)
        .bind(author_id)
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

fn merge_payload(s: &DetectedSuggestion) -> (&[i64], &[String], i64, &str) {
    match &s.payload {
        CleanupPayload::Merge {
            source_ids,
            source_names,
            canonical_id,
            canonical_name,
        } => (source_ids, source_names, *canonical_id, canonical_name),
        other => panic!("expected a Merge payload, got {other:?}"),
    }
}
