//! The device token exchange (`/v1/auth/device` and `/v1/auth/refresh`) and
//! the extractor-level guards every Kobo route shares — token revocation and
//! the path-uuid length cap.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use omnibus_db as db;
use tower::ServiceExt;

use super::super::*;
use super::{body_json, fixture, get};

#[tokio::test]
async fn auth_device_returns_a_bearer_envelope() {
    let (app, _pool, token, _uid) = fixture().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/auth/device"))
                .method("POST")
                .header("host", "omni.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["TokenType"], "Bearer");
    for field in ["AccessToken", "RefreshToken", "TrackingId", "UserKey"] {
        assert!(
            json[field].as_str().is_some_and(|s| !s.is_empty()),
            "{field} should be present and non-empty"
        );
    }
}

#[test]
fn auth_envelope_values_are_pinned_to_a_fixed_digest() {
    // Hard-coded rather than recomputed: the whole point is that these values
    // survive a toolchain bump. A hash-function swap that silently rotated
    // every device's envelope would pass a self-consistent test but fail here.
    let env = dto::auth_envelope("tok123");

    // == sha256(b"access" + b"\x00" + b"tok123"), verified independently.
    assert_eq!(
        env.access_token,
        "ecaf3802245d9f7c9914df62bdb3baa89388b6a6cb08d4b703dcb696c6bcd220"
    );
    assert_eq!(env.token_type, "Bearer");
    assert_ne!(env.access_token, env.refresh_token);
    assert_ne!(env.tracking_id, env.user_key);
}

#[tokio::test]
async fn auth_refresh_returns_the_same_envelope_as_the_initial_exchange() {
    // The device refreshes on a schedule; a value that changed between the two
    // would look like a rotated credential it needs to act on.
    let (app, _pool, token, _uid) = fixture().await;

    let device = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/auth/device"))
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let refresh = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/auth/refresh"))
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(body_json(device).await, body_json(refresh).await);
}

#[tokio::test]
async fn a_revoked_token_is_rejected() {
    let (app, pool, token, uid) = fixture().await;
    // Revoke the only device this token belongs to.
    let dev = db::kobo_devices::list_devices(&pool, uid).await.unwrap();
    db::kobo_devices::revoke_device(&pool, uid, dev[0].id)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn uuid_routes_reject_an_oversized_path_uuid() {
    let (app, _pool, token, _uid) = fixture().await;
    let oversized = "a".repeat(omnibus_shared::BOOK_UUID_MAX_LEN + 1);

    for uri in [
        format!("/kobo/{token}/v1/library/{oversized}/metadata"),
        format!("/kobo/{token}/v1/download/{oversized}"),
        format!("/kobo/{token}/v1/books/{oversized}/thumbnail/400/600/100/false/image.jpg"),
    ] {
        let res = app.clone().oneshot(get(uri.clone())).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "GET {uri}");
    }

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/library/{oversized}/state"))
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "ReadingStates": [] }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
