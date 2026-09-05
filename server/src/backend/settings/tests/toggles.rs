//! The admin toggles: self-registration closed and reopened (with a
//! second user signing up afterwards) and the MCP endpoint switch, each
//! rejecting non-admins and anonymous callers.

use axum::{
    body::Body,
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Build an admin-authenticated `POST /api/settings/registration` request.
fn post_registration_req(token: &str, enabled: bool) -> Request<Body> {
    Request::builder()
        .uri("/api/settings/registration")
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({ "enabled": enabled }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn api_post_registration_closes_self_registration_for_admin() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    omnibus_db::auth::set_registration_enabled(&pool, true)
        .await
        .unwrap();

    let res = app
        .oneshot(post_registration_req(&token, false))
        .await
        .expect("request should succeed");

    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(
        !omnibus_db::auth::registration_enabled(&pool).await.unwrap(),
        "the switch must have reached the settings row"
    );
}

#[tokio::test]
async fn api_post_registration_reopens_self_registration_for_admin() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    omnibus_db::auth::set_registration_enabled(&pool, false)
        .await
        .unwrap();

    let res = app
        .oneshot(post_registration_req(&token, true))
        .await
        .expect("request should succeed");

    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(omnibus_db::auth::registration_enabled(&pool).await.unwrap());
}

#[tokio::test]
async fn api_post_registration_rejects_non_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    omnibus_db::auth::set_registration_enabled(&pool, false)
        .await
        .unwrap();

    let res = app
        .oneshot(post_registration_req(&token, true))
        .await
        .expect("request should succeed");

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(
        !omnibus_db::auth::registration_enabled(&pool).await.unwrap(),
        "a rejected request must not have changed the setting"
    );
}

#[tokio::test]
async fn api_post_registration_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let body = serde_json::json!({ "enabled": true });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/settings/registration")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_registration_returns_500_on_db_failure() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    sqlx::query("DROP TABLE settings")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(post_registration_req(&token, true))
        .await
        .expect("request should succeed");

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn reopening_registration_through_the_api_lets_a_second_user_sign_up() {
    // The acceptance criterion for #909: an admin must be able to onboard
    // user #2 through the running app, with no direct SQL and no
    // OMNIBUS_INITIAL_ADMIN boot hook. `create_user` is what
    // `POST /api/auth/register` calls, so driving it here proves the gate the
    // endpoint just moved is the one self-registration actually consults.
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    omnibus_db::auth::set_registration_enabled(&pool, false)
        .await
        .unwrap();

    let refused = omnibus_db::auth::create_user(&pool, "bob", "correct horse battery staple").await;
    assert!(
        matches!(
            refused,
            Err(omnibus_db::auth::AuthError::RegistrationDisabled)
        ),
        "precondition: signup must be closed before the admin reopens it"
    );

    let res = app
        .oneshot(post_registration_req(&token, true))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    omnibus_db::auth::create_user(&pool, "bob", "correct horse battery staple")
        .await
        .expect("a second user must be able to self-register once reopened");
}

/// Build an authenticated `POST /api/settings/mcp` request.
fn post_mcp_req(token: &str, enabled: bool) -> Request<Body> {
    Request::builder()
        .uri("/api/settings/mcp")
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({ "enabled": enabled }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn api_settings_mcp_round_trips_for_admin() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Default reads disabled.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/settings/mcp")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let status: omnibus_shared::McpStatus = serde_json::from_slice(&bytes).unwrap();
    assert!(!status.enabled);

    // Enable, then the setting row reflects it.
    let res = app.oneshot(post_mcp_req(&token, true)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(omnibus_db::mcp_enabled(&pool).await.unwrap());
}

#[tokio::test]
async fn api_settings_mcp_rejects_non_admin_and_anonymous() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(post_mcp_req(&token, true))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(
        !omnibus_db::mcp_enabled(&pool).await.unwrap(),
        "a rejected request must not have changed the setting"
    );

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/settings/mcp")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "the read is admin-gated too"
    );

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/settings/mcp")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "enabled": true }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
