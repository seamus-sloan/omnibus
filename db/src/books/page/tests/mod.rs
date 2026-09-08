//! Keyset pagination tests for `list_books_page`, split by sub-topic into
//! the sibling modules below; the library, book and physical-copy insert
//! fixtures they share live here.

mod filters;
mod overrides;
mod paging;

use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::SqlitePool;

use super::*;

/// Unique uuid/scan_key per inserted row.
fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("uuid-{}", N.fetch_add(1, Ordering::Relaxed))
}

async fn insert_lib(pool: &SqlitePool, path: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib') RETURNING id")
        .bind(path)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Insert a book with an explicit `(title, sort, series_sort, series_index)`
/// plus a backing `book_files` row (so the fileless filter keeps it). Returns the
/// new book id.
async fn insert_book(
    pool: &SqlitePool,
    lib_id: i64,
    title: &str,
    sort: Option<&str>,
    series_sort: Option<&str>,
    series_index: Option<f64>,
) -> i64 {
    let key = uniq();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort, series_sort, series_index)
         VALUES (?, ?, ?, '/p', ?, ?, ?, ?) RETURNING id",
    )
    .bind(&key)
    .bind(&key)
    .bind(lib_id)
    .bind(title)
    .bind(sort)
    .bind(series_sort)
    .bind(series_index)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'EPUB', ?, 1, 1)",
    )
    .bind(id)
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn ids(page: &BookPage) -> Vec<i64> {
    page.books.iter().map(|b| b.id).collect()
}

fn titles(page: &BookPage) -> Vec<String> {
    page.books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect()
}
