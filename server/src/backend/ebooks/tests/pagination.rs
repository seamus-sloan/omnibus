//! Keyset pagination on `GET /api/ebooks`: the `X-Next-Cursor` header, its
//! absence at end of stream, and the 400s a malformed or unaccompanied
//! cursor earns.

use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::Settings;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;

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
            word_count: None,
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
            scan_interval_hours: None,
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
            scan_interval_hours: None,
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
            scan_interval_hours: None,
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
