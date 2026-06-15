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
        INSERT INTO books (uuid, library_id, path, title, sort)
        SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
               'Title ' || printf('%010d', i)
          FROM n
        ",
    )
    .bind(total)
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
