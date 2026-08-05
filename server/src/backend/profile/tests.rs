//! Integration tests for the profile routes: display-name writes (and the
//! wishlist label they rename), avatar upload/serve/delete, the conditional
//! revalidation path, and the `?token=` media auth the `<img>` fetches need.

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support::{bearer_token, create_user};
use crate::backend::test_support::{
    fixture, get_anon, get_with_bearer, get_with_bearer_and_if_none_match,
};

const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0];
const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";

/// Multipart body carrying one `avatar` field, as the upload route expects.
fn avatar_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----omnibus-test-avatar-boundary";
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"avatar\"; filename=\"avatar.png\"\r\n",
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn post_avatar_request(token: &str, content_type: &str, bytes: &[u8]) -> Request<Body> {
    let (ct, body) = avatar_multipart(content_type, bytes);
    Request::builder()
        .uri("/api/account/avatar")
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body))
        .unwrap()
}

fn post_profile_request(token: &str, json: &str) -> Request<Body> {
    Request::builder()
        .uri("/api/account/profile")
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

// ── Display name ─────────────────────────────────────────────────────

#[tokio::test]
async fn post_profile_sets_the_display_name_and_surfaces_it_on_me() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "cool-guy-7").await;
    let token = bearer_token(&pool, user.id).await;

    let resp = app
        .clone()
        .oneshot(post_profile_request(&token, r#"{"display_name":"Seamus"}"#))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = db::auth::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.display_name.as_deref(), Some("Seamus"));
    assert_eq!(stored.username, "cool-guy-7");
}

#[tokio::test]
async fn post_profile_renames_the_callers_wishlist_shelf() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "cool-guy-7").await;
    let token = bearer_token(&pool, user.id).await;
    // The test helper inserts the row directly, bypassing `create_user`'s
    // inline provisioning; the boot backfill is what a real instance runs.
    db::provision_wishlist_shelves(&pool).await.unwrap();

    app.clone()
        .oneshot(post_profile_request(&token, r#"{"display_name":"Seamus"}"#))
        .await
        .unwrap();

    let name: String = sqlx::query_scalar(
        "SELECT name FROM shelves WHERE owner_user_id = ? AND kind = 'wishlist'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(name, "Seamus's Wishlist");
}

#[tokio::test]
async fn post_profile_clears_the_display_name_when_null() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;
    db::auth::set_display_name(&pool, user.id, Some("Alice A."))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(post_profile_request(&token, r#"{"display_name":null}"#))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(db::auth::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap()
        .display_name
        .is_none());
}

#[tokio::test]
async fn post_profile_rejects_an_over_long_display_name_with_422() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;

    let too_long = "a".repeat(db::auth::DISPLAY_NAME_MAX_LEN + 1);
    let resp = app
        .clone()
        .oneshot(post_profile_request(
            &token,
            &format!(r#"{{"display_name":"{too_long}"}}"#),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn post_profile_rejects_an_unauthenticated_caller() {
    let (app, _state, _pool) = fixture().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/account/profile")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"Mallory"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Avatar ───────────────────────────────────────────────────────────

#[tokio::test]
async fn post_avatar_stores_the_image_and_get_serves_it_with_an_etag() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;

    let resp = app
        .clone()
        .oneshot(post_avatar_request(&token, "image/png", PNG_MAGIC))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/users/{}/avatar", user.id),
            &token,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(resp.headers()[header::VARY], "Cookie, Authorization");
    assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(resp.headers().contains_key(header::ETAG));
    // Strong tag (no `W/` prefix) — a weak one would rule out `If-Range`.
    assert!(!resp.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .starts_with("W/"));
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), PNG_MAGIC);
}

#[tokio::test]
async fn get_user_avatar_answers_304_for_a_matching_if_none_match() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;
    app.clone()
        .oneshot(post_avatar_request(&token, "image/png", PNG_MAGIC))
        .await
        .unwrap();

    let uri = format!("/api/users/{}/avatar", user.id);
    let first = app
        .clone()
        .oneshot(get_with_bearer(&uri, &token))
        .await
        .unwrap();
    let etag = first.headers()[header::ETAG].to_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(get_with_bearer_and_if_none_match(&uri, &token, &etag))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    // The 304 must carry `Vary` too, or a shared cache could hand this
    // response to a differently-authenticated request on the same URL.
    assert_eq!(resp.headers()[header::VARY], "Cookie, Authorization");
    assert!(resp.headers().contains_key(header::CACHE_CONTROL));
}

#[tokio::test]
async fn post_avatar_replaces_the_previous_image_and_changes_the_etag() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;
    let uri = format!("/api/users/{}/avatar", user.id);

    app.clone()
        .oneshot(post_avatar_request(&token, "image/png", PNG_MAGIC))
        .await
        .unwrap();
    let first = app
        .clone()
        .oneshot(get_with_bearer(&uri, &token))
        .await
        .unwrap();
    let first_etag = first.headers()[header::ETAG].to_str().unwrap().to_string();

    app.clone()
        .oneshot(post_avatar_request(&token, "image/jpeg", JPEG_MAGIC))
        .await
        .unwrap();
    let second = app
        .clone()
        .oneshot(get_with_bearer(&uri, &token))
        .await
        .unwrap();

    assert_eq!(second.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_ne!(
        second.headers()[header::ETAG].to_str().unwrap(),
        first_etag,
        "replacing the bytes must move the validator"
    );
}

#[tokio::test]
async fn post_avatar_rejects_an_svg() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;

    let resp = app
        .clone()
        .oneshot(post_avatar_request(&token, "image/svg+xml", SVG))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_avatar_rejects_a_non_image_content_type() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;

    let resp = app
        .clone()
        .oneshot(post_avatar_request(&token, "text/plain", b"not an image"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_user_avatar_is_404_for_a_user_without_one() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;

    let resp = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/users/{}/avatar", user.id),
            &token,
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_avatar_removes_it_and_later_reads_404() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;
    app.clone()
        .oneshot(post_avatar_request(&token, "image/png", PNG_MAGIC))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/account/avatar")
                .method("DELETE")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/users/{}/avatar", user.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_user_avatar_accepts_a_query_token_and_rejects_anonymous() {
    let (app, _state, pool) = fixture().await;
    let user = create_user(&pool, "alice").await;
    let token = bearer_token(&pool, user.id).await;
    app.clone()
        .oneshot(post_avatar_request(&token, "image/png", PNG_MAGIC))
        .await
        .unwrap();

    // `<img src>` from the mobile WebView can only carry the session as a
    // query token — the gate's media allowlist has to include this route.
    let resp = app
        .clone()
        .oneshot(get_anon(&format!(
            "/api/users/{}/avatar?token={token}",
            user.id
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(get_anon(&format!("/api/users/{}/avatar", user.id)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn avatar_writes_are_scoped_to_the_caller() {
    let (app, _state, pool) = fixture().await;
    let alice = create_user(&pool, "alice").await;
    let bob = create_user(&pool, "bob").await;
    let alice_token = bearer_token(&pool, alice.id).await;

    app.clone()
        .oneshot(post_avatar_request(&alice_token, "image/png", PNG_MAGIC))
        .await
        .unwrap();

    // The write route takes no user id at all, so alice's upload can only
    // ever land on alice.
    assert!(db::auth::get_user_avatar(&pool, alice.id)
        .await
        .unwrap()
        .is_some());
    assert!(db::auth::get_user_avatar(&pool, bob.id)
        .await
        .unwrap()
        .is_none());
}
