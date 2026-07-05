//! Integration tests for the ebook endpoints — `GET /api/library`,
//! `GET /api/ebooks` (listing + pagination headers), `GET /api/ebooks/{uuid}`
//! (single-book metadata), and `GET /api/ebooks/{uuid}/file` (raw EPUB
//! serving). Covers auth gating, 4xx client errors, and 5xx DB-failure paths.

use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::Settings;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_library_returns_empty_sections_when_paths_not_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/library", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
    assert!(contents.ebooks.path.is_none());
    assert_eq!(contents.ebooks.total_files, 0);
    assert!(contents.audiobooks.path.is_none());
    assert_eq!(contents.audiobooks.total_files, 0);
}

#[tokio::test]
async fn api_get_library_reports_error_for_nonexistent_path() {
    let (_, _, pool) = fixture().await;
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/does/not/exist/omnibus_test".to_string()),
            audiobook_library_path: None,
        },
    )
    .await
    .expect("set should succeed");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = crate::backend::rest_router(AppState::new(pool));

    let response = app
        .oneshot(get_with_bearer("/api/library", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
    assert!(contents.ebooks.error.is_some());
    assert!(contents.audiobooks.path.is_none());
}

#[tokio::test]
async fn api_get_ebooks_returns_empty_when_path_not_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert!(lib.path.is_none());
    assert!(lib.books.is_empty());
    assert!(lib.error.is_none());
}

#[tokio::test]
async fn api_get_ebooks_returns_empty_library_for_configured_path_without_index() {
    // /api/ebooks now reads from the books table; an unindexed path
    // surfaces as an empty library at that path, not an error.
    let pool = db::init_db("sqlite::memory:")
        .await
        .expect("db should initialize");
    let path = "/does/not/exist/omnibus_ebook_test";
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some(path.to_string()),
            audiobook_library_path: None,
        },
    )
    .await
    .expect("set should succeed");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = crate::backend::rest_router(AppState::new(pool));

    let response = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(lib.path.as_deref(), Some(path));
    assert!(lib.books.is_empty());
    assert!(lib.error.is_none());
}

#[tokio::test]
async fn api_get_ebooks_sets_total_count_header_with_indexed_library() {
    // Issue #81: every /api/ebooks response carries an X-Total-Count
    // header. When the result fits under MAX_BOOKS_RETURNED no
    // X-Total-Cap header is set, so the client knows the response
    // is complete.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    db::replace_books(
        &pool,
        "/lib",
        vec![
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("A".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            },
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "beta.epub".into(),
                    title: Some("B".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            },
        ],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("2"),
        "X-Total-Count must reflect the true row count"
    );
    assert!(
        response.headers().get("X-Total-Cap").is_none(),
        "X-Total-Cap must not be set when the response is not truncated"
    );
}

#[tokio::test]
async fn api_get_ebooks_sets_total_cap_header_when_truncated() {
    // Issue #81: when the underlying row count exceeds MAX_BOOKS_RETURNED,
    // the response body is capped and X-Total-Cap is attached so the
    // client knows the JSON it received isn't the full set.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();

    // Bulk-seed > MAX_BOOKS_RETURNED rows directly so the test runtime
    // stays in milliseconds. The cap behavior only needs rows to exist;
    // the indexer's full m2m wiring isn't relevant here.
    let total = db::MAX_BOOKS_RETURNED + 5;
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1
            UNION ALL
            SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
        SELECT 'uuid-' || i, 'b' || i || '.epub', ?, '/lib/b' || i, 'Title ' || i,
               'Title ' || printf('%010d', i)
          FROM n
        ",
    )
    .bind(total)
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    // Give every seeded book a `book_files` row so the F2 ghost filter
    // (which hides fileless books from list/count reads) keeps them counted.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         SELECT id, 'EPUB', 'b' || id, 1, 1 FROM books WHERE library_id = ?",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();

    let response = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok()),
        Some(total),
        "X-Total-Count must report the uncapped row count"
    );
    assert_eq!(
        response
            .headers()
            .get("X-Total-Cap")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok()),
        Some(db::MAX_BOOKS_RETURNED),
        "X-Total-Cap must equal MAX_BOOKS_RETURNED when the response is truncated"
    );
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

#[tokio::test]
async fn api_library_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/library")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_ebooks_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/ebooks")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// -------------------------------------------------------------------
// /api/ebooks/{uuid}/file — raw EPUB byte serving
// -------------------------------------------------------------------

#[tokio::test]
async fn api_get_ebook_file_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/file"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_file_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/does-not-exist/file", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_file_returns_200_with_epub_bytes() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Write a tiny stand-in EPUB file under a temp dir whose parent is
    // recorded as `books.path` and whose stem matches `book_files.filename`.
    // pid + nanos suffix (matching `backend.rs`'s covers scratch dir) so
    // concurrent/repeat runs don't race on the same path.
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_ebook_file_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let stem = "alpha";
    let file_path = tmp.join(format!("{stem}.epub"));
    std::fs::write(&file_path, b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "11111111-1111-1111-1111-111111111111";
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'Alpha')")
            .bind(uuid)
            .bind(lib_id)
            .bind(tmp.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/epub+zip"),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

/// Drives the first `internal(...)` arm of `get_ebook_file`: when
/// `db::resolve_book_id_by_uuid` returns `Err`, the handler must
/// surface 500 (not 404). The induce-sqlx-error idiom is to drop
/// the `books` table out from under the live pool after seeding the
/// auth gate's `users`/`sessions` tables — the gate keeps passing,
/// the handler's first query hits "no such table: books" and the
/// response collapses into the shared `internal()` envelope. Same
/// pattern is reusable for any future 5xx test that needs to
/// isolate a single db call without a mock layer.
#[tokio::test]
async fn api_get_ebook_file_returns_500_when_resolve_book_id_by_uuid_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // FK cascade: dependents of `books` (book_files, etc.) reference
    // it, so disable FK enforcement before dropping to keep the
    // single DROP statement minimal. We don't touch `users` /
    // `sessions`, so the auth gate's `lookup_session` still works.
    // `PRAGMA foreign_keys` is per-connection in SQLite, so acquire
    // one connection and pin both statements to it — `execute(&pool)`
    // would let the PRAGMA and the DROP land on different pool
    // connections and the DROP could then trip an FK error.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/any-uuid/file", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Drives the second `internal(...)` arm of `get_ebook_file`: when
/// `db::resolve_book_id_by_uuid` returns `Ok(Some(_))` but
/// `db::book_file_path` returns `Err`, the handler must surface 500.
/// Seed a book row so the first call succeeds, then drop the
/// `book_files` table so the second call's JOIN errors out.
#[tokio::test]
async fn api_get_ebook_file_returns_500_when_book_file_path_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "22222222-2222-2222-2222-222222222222";
    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'B')")
        .bind(uuid)
        .bind(lib_id)
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap();

    // FKs off because `book_file_parts` references `book_files`;
    // we want the single DROP to succeed without cascading further.
    // PRAGMA + DROP pinned to a single connection — see the sibling
    // test above for the rationale.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE book_files")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

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

#[tokio::test]
async fn api_get_ebook_kepub_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/kepub"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_kepub_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/does-not-exist/kepub", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Cache hit: a fresh KEPUB already on disk is served verbatim with a
/// `.kepub.epub` filename (no kepubify needed).
#[tokio::test]
async fn api_get_ebook_kepub_serves_cached_kepub_with_kepub_filename() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, book_id, tmp) = seed_epub_on_disk(&pool).await;

    let data_dir = DataDirGuard::new("kepub_hit");
    let cache = data_dir.path.join("kepub");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join(format!("{book_id}.kepub.epub")), b"KEPUB-CACHED").unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/kepub"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        content_disposition(&res),
        format!("attachment; filename=\"{uuid}.kepub.epub\""),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"KEPUB-CACHED");

    std::fs::remove_dir_all(&tmp).ok();
}

/// Fallback: with no cache and an EPUB kepubify can't convert (invalid input
/// or binary absent), the handler serves the plain EPUB with a `.epub`
/// filename rather than erroring.
#[tokio::test]
async fn api_get_ebook_kepub_falls_back_to_plain_epub() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _book_id, tmp) = seed_epub_on_disk(&pool).await;

    // Empty cache dir → conversion is attempted and fails on the fake EPUB.
    let _data_dir = DataDirGuard::new("kepub_fallback");

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/kepub"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        content_disposition(&res),
        format!("attachment; filename=\"{uuid}.epub\""),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

// -------------------------------------------------------------------
// /api/ebooks/{uuid}/download — raw EPUB download (attachment)
// -------------------------------------------------------------------

#[tokio::test]
async fn api_get_ebook_download_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/download"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_download_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/ebooks/does-not-exist/download",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_download_returns_200_with_attachment_disposition() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_ebook_download_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let stem = "alpha";
    let file_path = tmp.join(format!("{stem}.epub"));
    std::fs::write(&file_path, b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "33333333-3333-3333-3333-333333333333";
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'Alpha')")
            .bind(uuid)
            .bind(lib_id)
            .bind(tmp.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/download"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/epub+zip"),
    );
    let disposition = res
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        disposition.starts_with("attachment;"),
        "download must force an attachment disposition, got {disposition:?}"
    );
    assert!(
        disposition.contains("alpha.epub"),
        "disposition should suggest the on-disk filename, got {disposition:?}"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

// -------------------------------------------------------------------
// F5b — keyset pagination on GET /api/ebooks
// -------------------------------------------------------------------

/// Index `count` books titled `Title 000`.. under `path` so their `sort`
/// column orders numerically (zero-padded).
async fn seed_titled_books(pool: &sqlx::SqlitePool, path: &str, count: usize) {
    let books: Vec<db::ebook::IndexedBook> = (0..count)
        .map(|i| db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: format!("book{i:03}.epub"),
                title: Some(format!("Title {i:03}")),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        })
        .collect();
    db::replace_books(pool, path, books).await.unwrap();
}

#[tokio::test]
async fn api_get_ebooks_paginates_via_cursor_and_emits_next_cursor_header() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    seed_titled_books(&pool, "/lib", 5).await;

    // Walk every page of size 2 by following X-Next-Cursor to exhaustion.
    let mut titles: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let uri = match &cursor {
            Some(c) => format!("/api/ebooks?sort=title&dir=asc&limit=2&cursor={c}"),
            None => "/api/ebooks?sort=title&dir=asc&limit=2".to_string(),
        };
        let resp = app
            .clone()
            .oneshot(get_with_bearer(&uri, &token))
            .await
            .expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);
        let next = resp
            .headers()
            .get("X-Next-Cursor")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(
            lib.books.len() <= 2,
            "page never exceeds the requested limit"
        );
        titles.extend(
            lib.books
                .iter()
                .map(|b| b.title.clone().unwrap_or_default()),
        );
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        titles,
        vec![
            "Title 000",
            "Title 001",
            "Title 002",
            "Title 003",
            "Title 004"
        ],
        "cursor walk reassembles the full ordered library with no overlap"
    );
}

#[tokio::test]
async fn api_get_ebooks_omits_next_cursor_header_at_end_of_stream() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    seed_titled_books(&pool, "/lib", 3).await;

    // A limit at/above the row count returns everything and no cursor.
    let resp = app
        .oneshot(get_with_bearer("/api/ebooks?sort=title&limit=50", &token))
        .await
        .expect("request should succeed");
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("X-Next-Cursor").is_none(),
        "no X-Next-Cursor when the page is the whole stream"
    );
    assert_eq!(
        resp.headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("3"),
        "paged response still reports the true total"
    );
    assert!(
        resp.headers().get("X-Total-Cap").is_none(),
        "a keyset page is never X-Total-Cap-truncated (per-page clamp only)"
    );
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(lib.books.len(), 3);
}

#[tokio::test]
async fn api_get_ebooks_rejects_malformed_cursor_with_400() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // Dots aren't in the base64url alphabet, so decode fails → client error.
    // sort+dir are supplied so we exercise the decode path, not the
    // missing-axis guard below.
    let resp = app
        .oneshot(get_with_bearer(
            "/api/ebooks?sort=title&dir=asc&cursor=not.a.cursor",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_ebooks_rejects_cursor_without_sort_and_dir_with_400() {
    // A cursor is decoded relative to the request's sort axis, so it's a hard
    // error to send one without an explicit sort+dir (would mis-position).
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let resp = app
        .oneshot(get_with_bearer("/api/ebooks?cursor=abc", &token))
        .await
        .expect("request should succeed");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_ebooks_without_pagination_params_omits_next_cursor() {
    // Backward-compat: the param-less call returns the full library and no
    // X-Next-Cursor, exactly as the pre-F5b handler did.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
        },
    )
    .await
    .unwrap();
    seed_titled_books(&pool, "/lib", 3).await;

    let resp = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .expect("request should succeed");
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("X-Next-Cursor").is_none());
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(lib.books.len(), 3);
}
