//! `POST /api/admin/rewrite-all-epubs` — the fleet-wide bulk admin action.
//! Dispatches `Task::RewriteAllEpubs` to the shared worker and returns as
//! soon as it's queued, so these just prove the route is wired up,
//! admin-gated, and returns immediately with no body. Count-accuracy and
//! the batched DB resolution are covered at the db layer (`epub_rewrite::tests`).

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn post_rewrite_all_epubs_returns_401_when_anonymous() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/rewrite-all-epubs")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_rewrite_all_epubs_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/rewrite-all-epubs")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_rewrite_all_epubs_returns_200_with_an_empty_body_once_queued() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    // A book exists but carries no active override — queuing the task must
    // still succeed immediately; the worker itself decides there's nothing
    // to rewrite once it runs.
    seed_book(&pool, "/lib", "Untouched").await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/rewrite-all-epubs")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert!(
        bytes.is_empty(),
        "the fire-and-forget response carries no body, got {bytes:?}"
    );
}
