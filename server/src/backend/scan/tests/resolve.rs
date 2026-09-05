//! The resolution routes — `resolve`, `search` and `resolve-meta`: auth,
//! the invalid / oversized inputs, the exact-ISBN library hit (unowned or
//! on the caller's wishlist), and the error mapping that answers a
//! provider outage with 503 but a DB failure with 500.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::ScanOutcome;
use tower::ServiceExt;

use super::{body_string, external_meta_json, json_body, post, seed_book_with_isbn, ISBN};
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_scan_resolve_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/resolve")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "isbn": ISBN }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_resolve_rejects_invalid_isbn() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": "12345" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_scan_resolve_rejects_an_oversized_isbn() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let oversized = "1".repeat(omnibus_shared::scan::ISBN_MAX_LEN + 1);
    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": oversized }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_scan_resolve_exact_hit_returns_in_library_unowned() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": ISBN }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    match json_body::<ScanOutcome>(res).await {
        ScanOutcome::InLibraryUnowned { book } => assert_eq!(book.uuid, uuid),
        other => panic!("expected InLibraryUnowned, got {other:?}"),
    }
}

#[tokio::test]
async fn api_scan_resolve_exact_hit_returns_on_wishlist_for_wishlister() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    omnibus_db::add_wishlist_entry(&pool, user.id, &uuid, WishlistSource::Manual)
        .await
        .unwrap();

    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": ISBN }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    match json_body::<ScanOutcome>(res).await {
        ScanOutcome::OnWishlist { book } => assert_eq!(book.uuid, uuid),
        other => panic!("expected OnWishlist, got {other:?}"),
    }
}

// The search happy path needs a live provider, so it lives in the
// `omnibus_db::metadata_lookup` wiremock tests; resolve-meta's exact-ISBN
// rung answers before any provider call, keeping its happy path
// network-free here.
#[tokio::test]
async fn api_scan_search_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/search")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "query": "dune" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_search_rejects_a_blank_query() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/scan/search",
            &token,
            serde_json::json!({ "query": "   " }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res).await;
    assert!(body.contains("query"), "got: {body}");
}

#[tokio::test]
async fn api_scan_search_rejects_an_oversized_query() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/scan/search",
            &token,
            serde_json::json!({
                "query": "x".repeat(omnibus_shared::scan::SEARCH_QUERY_MAX_LEN + 1),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_scan_resolve_meta_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/resolve-meta")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "meta": external_meta_json("Some Book", ISBN) })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_resolve_meta_rejects_an_oversized_meta_title() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized_title =
        "x".repeat(omnibus_shared::metadata_lookup::ExternalBookMeta::TITLE_MAX_LEN + 1);
    let res = app
        .oneshot(post(
            "/api/scan/resolve-meta",
            &token,
            serde_json::json!({ "meta": external_meta_json(&oversized_title, ISBN) }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res).await;
    assert!(body.contains("title"), "got: {body}");
}

#[tokio::test]
async fn api_scan_resolve_meta_exact_hit_returns_in_library_unowned() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/resolve-meta",
            &token,
            serde_json::json!({ "meta": external_meta_json("Effective Java", ISBN) }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    match json_body::<ScanOutcome>(res).await {
        ScanOutcome::InLibraryUnowned { book } => assert_eq!(book.uuid, uuid),
        other => panic!("expected InLibraryUnowned, got {other:?}"),
    }
}

#[test]
fn scan_error_maps_a_provider_outage_to_503_not_500() {
    use omnibus_db::{MetadataLookupError, ScanError};

    // Both providers down is an outage, not a bug: a 500 tells the reader
    // Omnibus is broken, when the honest answer is "try again later".
    let err = ScanError::Lookup(MetadataLookupError::Provider(anyhow::anyhow!(
        "google books returned an error status"
    )));
    assert_eq!(
        super::super::scan_error("scan_resolve", err).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn scan_error_still_maps_a_db_failure_to_500() {
    use omnibus_db::ScanError;

    let err = ScanError::Sqlx(sqlx::Error::RowNotFound);
    assert_eq!(
        super::super::scan_error("scan_resolve", err).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
