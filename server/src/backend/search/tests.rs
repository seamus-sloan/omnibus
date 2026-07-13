//! Tests for search and command-palette handlers.
use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::Settings;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;
use crate::backend::SEARCH_RATE_LIMIT_MAX;

#[tokio::test]
async fn api_get_search_sets_total_count_header_with_indexed_library() {
    // Issue #81: /api/search must attach X-Total-Count on every response,
    // matching the /api/ebooks contract.
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
                    title: Some("Alpha".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            },
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "beta.epub".into(),
                    title: Some("Beta".into()),
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
        .oneshot(get_with_bearer("/api/search?q=Alpha", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "X-Total-Count must reflect the FTS match count"
    );
    assert!(
        response.headers().get("X-Total-Cap").is_none(),
        "X-Total-Cap must not be set when search results fit under the cap"
    );
}

#[tokio::test]
async fn api_get_search_sets_total_count_zero_when_path_not_configured() {
    // Issue #81: the early-return path (no library configured) must
    // still attach X-Total-Count: 0 so the client can rely on the
    // header always being present.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/search?q=anything", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("0"),
        "X-Total-Count must be 0 on the no-library-configured early return"
    );
    assert!(
        response.headers().get("X-Total-Cap").is_none(),
        "X-Total-Cap must not be set on the early-return path"
    );
}

#[tokio::test]
async fn api_search_returns_empty_when_path_not_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/search?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert!(lib.path.is_none());
    assert!(lib.books.is_empty());
}

#[tokio::test]
async fn api_search_rejects_missing_q_param() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/search", &token))
        .await
        .expect("request should succeed");
    // axum's Query extractor returns 400 for missing required fields.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_search_rejects_over_length_q_with_400() {
    // Issue #638: the handler caps the decoded `q` length at
    // MAX_SEARCH_QUERY_LEN bytes and rejects longer input with 400 before any
    // search db call.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized = "a".repeat(MAX_SEARCH_QUERY_LEN + 1);
    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/search?q={oversized}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_search_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/search?q=hello")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// -------------------------------------------------------------------
// /api/search/palette — search palette (F1.5)
// -------------------------------------------------------------------

#[tokio::test]
async fn api_search_palette_returns_empty_when_path_not_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::PaletteResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.books.is_empty());
    assert!(results.authors.is_empty());
}

#[tokio::test]
async fn api_search_palette_rejects_over_length_q_with_400() {
    // Issue #638: the palette handler shares the same MAX_SEARCH_QUERY_LEN
    // cap, rejecting over-length input with 400 before any search db call.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized = "a".repeat(MAX_SEARCH_QUERY_LEN + 1);
    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/search/palette?q={oversized}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_search_palette_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/search/palette?q=hello"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_search_palette_returns_429_after_budget_exceeded() {
    // Issue #124: /api/search/* runs four heavy FTS5 queries per request,
    // so it gets a per-IP fixed-window rate limit. The limit is set to
    // SEARCH_RATE_LIMIT_MAX requests per SEARCH_RATE_LIMIT_WINDOW; the
    // (SEARCH_RATE_LIMIT_MAX + 1)th request from the same principal must
    // be rejected with 429.
    //
    // `oneshot` requests carry no `ConnectInfo<SocketAddr>` extension, so
    // the limiter's IP fallback (`0.0.0.0`) applies — every request in
    // this test shares one bucket, which is exactly what we want.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    for i in 0..SEARCH_RATE_LIMIT_MAX {
        let res = app
            .clone()
            .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "request #{} should be within budget",
            i + 1
        );
    }

    // The (MAX+1)th request must trip the limiter.
    let over_limit = app
        .clone()
        .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(
        over_limit.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "request beyond SEARCH_RATE_LIMIT_MAX must return 429",
    );
}
