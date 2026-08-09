//! Integration tests for `GET /api/admin/health`: `AdminUser` gating (401
//! anonymous / 403 non-admin) and a 200 admin response carrying every
//! section of the report.

use axum::{body::to_bytes, http::StatusCode};
use omnibus_db::test_support::seed_minimal_books;
use omnibus_shared::admin_health::AdminHealthReport;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn get_admin_health_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/admin/health"))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_admin_health_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/admin/health", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_admin_health_returns_200_with_every_section_for_an_admin() {
    let (app, _, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    seed_minimal_books(&pool, 2).await;

    let res = app
        .oneshot(get_with_bearer("/api/admin/health", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let report: AdminHealthReport = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(report.index.book_count, 2);
    // `seed_minimal_books` bypasses the indexer, so `books_fts` never gets
    // populated — a real drift the FTS section is meant to catch.
    assert!(!report.fts.in_sync);
    // No worker tasks were posted on this fixture's own `Worker`, so the
    // queue section is present but empty, not absent. `last_errors` is
    // intentionally *not* asserted here: `db::error_ring` is one
    // process-wide buffer shared with `logging::error_ring_layer`'s own
    // tests, which legitimately record real entries into it concurrently.
    assert!(report.worker_queue.is_empty());
}
