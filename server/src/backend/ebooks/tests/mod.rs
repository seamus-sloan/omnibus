//! Integration tests for the `/api/library`, `/api/ebooks`, and ebook
//! byte-serving routes, driving `rest_router` via `oneshot` against an
//! in-memory DB. The shared on-disk fixtures and the per-book detail route
//! live here; the remaining endpoints are split into the sibling modules
//! below.

mod conditional;
mod download;
mod file;
mod format_filters;
mod listing;
mod pages;
mod pagination;

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::*;

// -------------------------------------------------------------------
// F4.1 — GET /api/ebooks/{uuid}/kepub (Send to Kobo download)
// -------------------------------------------------------------------

/// Seed one EPUB book with a real file on disk. Returns `(uuid, book_id, tmp)`;
/// caller removes `tmp`. `last_modified` is backdated so a freshly-written
/// KEPUB cache always reads as fresh.
async fn seed_epub_on_disk(pool: &sqlx::SqlitePool) -> (String, i64, std::path::PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_kepub_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("alpha.epub"), b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "33333333-3333-3333-3333-333333333333".to_string();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, last_modified) \
         VALUES (?, ?, ?, 'Alpha', '2000-01-01 00:00:00')",
    )
    .bind(&uuid)
    .bind(lib_id)
    .bind(tmp.to_str().unwrap())
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query("INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', 'alpha', 0)")
        .bind(book_id)
        .execute(pool)
        .await
        .unwrap();
    (uuid, book_id, tmp)
}

fn content_disposition(res: &axum::response::Response) -> String {
    res.headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

// -------------------------------------------------------------------
// GET /api/ebooks/{uuid}/pages/{n} — CBZ page extraction
// -------------------------------------------------------------------

/// Seed one CBZ book whose archive holds two stored pages plus macOS
/// AppleDouble junk that must not count as a page. Returns `(uuid, tmp)`;
/// caller removes `tmp`.
async fn seed_cbz_on_disk(pool: &sqlx::SqlitePool) -> (String, std::path::PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_cbz_page_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cbz = db::test_support::build_stored_zip(&[
        ("p2.png", b"page-two"),
        ("p1.jpg", b"page-one"),
        ("__MACOSX/._p1.jpg", b"appledouble junk"),
    ]);
    std::fs::write(tmp.join("alpha.cbz"), cbz).unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "55555555-5555-5555-5555-555555555555".to_string();
    // `page_count` is stamped here rather than derived at request time
    // (#1593) — two real pages (p1.jpg, p2.png); the AppleDouble junk entry
    // never counts, same as the indexer's own count would produce.
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, page_count) \
         VALUES (?, ?, ?, 'Alpha', 2)",
    )
    .bind(&uuid)
    .bind(lib_id)
    .bind(tmp.to_str().unwrap())
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'CBZ', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    (uuid, tmp)
}

#[tokio::test]
async fn api_get_ebook_returns_200_with_metadata() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    db::replace_books(
        &pool,
        "/lib",
        vec![db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: "alpha.epub".into(),
                title: Some("Alpha Book".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();

    let books = db::list_books(&pool, "/lib").await.unwrap();
    let id = books[0].id;
    let uuid = books[0].unique_identifier.clone().unwrap();

    let response = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(book.title.as_deref(), Some("Alpha Book"));
    assert_eq!(book.id, id);
}

#[tokio::test]
async fn api_get_ebook_returns_404_for_unknown_id() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/ebooks/9999", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_returns_401_when_anonymous() {
    let (app, _state, _pool) = fixture().await;
    let response = app
        .oneshot(get_anon("/api/ebooks/1"))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The detail payload carries `page_count` for a CBZ book so the pager can
/// render its slider without a second request.
#[tokio::test]
async fn api_get_ebook_returns_page_count_for_cbz_book() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        book.page_count,
        Some(2),
        "two real pages; AppleDouble junk never counts"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
