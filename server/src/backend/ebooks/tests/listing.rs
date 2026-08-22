//! `GET /api/library` and the unpaginated `GET /api/ebooks` listing: the
//! empty and misconfigured-path responses, the total-count and cap headers,
//! and the anonymous 401s.

use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::Settings;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;

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
            scan_interval_hours: None,
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
            scan_interval_hours: None,
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
            scan_interval_hours: None,
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
                word_count: None,
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
                word_count: None,
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
            scan_interval_hours: None,
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
