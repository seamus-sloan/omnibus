//! Integration tests for the top-level `/api/*` router (health, body limits, rate limiting).
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

// -------------------------------------------------------------------
// /api/_health — unauthenticated liveness + fingerprint.
// -------------------------------------------------------------------

#[tokio::test]
async fn api_health_returns_200_unauth_with_app_and_build_id() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/_health"))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON body");
    assert_eq!(body["app"], "omnibus");
    assert_eq!(body["status"], "ok");
    let build_id = body["build_id"]
        .as_str()
        .expect("build_id should be string");
    assert!(
        build_id.chars().all(|c| c.is_ascii_digit()),
        "build_id should be all digits, got {build_id:?}"
    );
    // `repo_root` is the workspace-identity field scripts/dev-server-up.sh
    // parses to distinguish this workspace's server from a sibling
    // jj workspace's server bound to the same port. Must be a string.
    assert!(
        body["repo_root"].is_string(),
        "repo_root must be a string, got {:?}",
        body["repo_root"]
    );
}

// -------------------------------------------------------------------
// Global request guards — protects against slow clients / oversized
// bodies holding a tokio worker indefinitely. See #85.
// -------------------------------------------------------------------

/// POSTing a JSON body well over the 1 MiB global cap should be
/// rejected with 413 PAYLOAD_TOO_LARGE before the handler ever sees
/// it. We pad the JSON with a long throwaway string so the body
/// exceeds the cap while staying syntactically valid.
#[tokio::test]
async fn api_post_settings_rejects_body_over_1mb_with_413() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // 2 MiB of filler — comfortably over the 1 MiB cap, but small
    // enough that allocating it in-test is cheap.
    let filler = "a".repeat(2 * 1024 * 1024);
    let body = serde_json::json!({
        "ebook_library_path": filler,
        "audiobook_library_path": "/books/audio"
    });
    let bytes = body.to_string();
    assert!(
        bytes.len() > 1024 * 1024,
        "test body must exceed the 1 MiB cap; got {} bytes",
        bytes.len()
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn api_upload_endpoints_share_per_ip_budget_and_exclude_reads() {
    // #168: the three binary-upload routes (cover POST, author-photo PUT,
    // photo-URL PUT) share ONE per-IP fixed-window limiter
    // (UPLOAD_RATE_LIMIT_MAX per UPLOAD_RATE_LIMIT_WINDOW) in
    // `upload_router`; the GET/DELETE photo routes live outside it and
    // carry no limiter. The limiter runs before the handler, so a
    // handler's own status doesn't matter — we only assert 429 vs not.
    // oneshot requests carry no ConnectInfo<SocketAddr>, so they all
    // share the limiter's 0.0.0.0 fallback bucket.
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let cover_post = || {
        Request::builder()
            .uri("/api/ebooks/1/cover")
            .method("POST")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from("x"))
            .unwrap()
    };
    let photo_put = || {
        Request::builder()
            .uri("/api/authors/1/photo")
            .method("PUT")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from("x"))
            .unwrap()
    };
    let photo_url_put = || {
        Request::builder()
            .uri("/api/authors/1/photo/url")
            .method("PUT")
            .header("content-type", "application/json")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(r#"{"url":"http://127.0.0.1:1/x.jpg"}"#))
            .unwrap()
    };

    // Spend the shared budget. Each is within budget, so none should 429.
    for i in 0..UPLOAD_RATE_LIMIT_MAX {
        let res = app
            .clone()
            .oneshot(photo_url_put())
            .await
            .expect("request should succeed");
        assert_ne!(
            res.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "request #{} should be within the shared upload budget",
            i + 1
        );
    }

    // Budget is now spent: every upload route trips the shared limiter,
    // proving the cap covers all three (not just photo-url).
    for (label, req) in [
        ("POST /api/ebooks/{id}/cover", cover_post()),
        ("PUT /api/authors/{id}/photo", photo_put()),
        ("PUT /api/authors/{id}/photo/url", photo_url_put()),
    ] {
        let res = app
            .clone()
            .oneshot(req)
            .await
            .expect("request should succeed");
        assert_eq!(
            res.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "{label} must return 429 once the shared upload budget is spent",
        );
    }

    // The read/non-upload photo routes are outside upload_router, so they
    // stay unthrottled even after the upload budget is exhausted.
    let get = app
        .clone()
        .oneshot(get_with_bearer("/api/authors/1/photo", &token))
        .await
        .expect("request should succeed");
    assert_ne!(
        get.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "GET author photo must not be rate-limited by the upload limiter",
    );
    let del = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/1/photo")
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_ne!(
        del.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "DELETE author photo must not be rate-limited by the upload limiter",
    );
}
