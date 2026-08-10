//! Integration tests for `GET /api/admin/health`: `AdminUser` gating (401
//! anonymous / 403 non-admin), a 200 admin response carrying every section
//! of the report, and the handler's own DB-failure (500) branch.

use axum::{body::to_bytes, extract::State, http::StatusCode};
use omnibus_db::{auth::SessionKind, test_support::seed_minimal_books};
use omnibus_shared::admin_health::AdminHealthReport;
use tower::ServiceExt;

use super::get_admin_health;
use crate::auth::test_support as auth_test_support;
use crate::auth::{AdminUser, AuthUser};
use crate::backend::test_support::*;

/// A minimal admin `AuthUser` for driving the handler directly (bypassing
/// the `AdminUser` extractor). Needed because `build_report`'s own
/// DB-failure branch shares the pool with the extractor's own session
/// lookup, so closing the pool before a routed request would fail
/// extraction rather than the handler body under test.
fn fake_admin(id: i64) -> AuthUser {
    AuthUser {
        id,
        username: "admin".to_string(),
        is_admin: true,
        can_upload: true,
        can_edit: true,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        session_id: 1,
        session_kind: SessionKind::Bearer,
    }
}

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

// ── DB-failure path (500) ────────────────────────────────────────────
//
// Invoked directly (rather than through `oneshot`) with a hand-built
// `AdminUser` and a pool closed only after the point at which the
// `AdminUser` extractor would have already run — see `fake_admin` above.

#[tokio::test]
async fn get_admin_health_returns_500_when_db_is_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = get_admin_health(AdminUser(fake_admin(1)), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
