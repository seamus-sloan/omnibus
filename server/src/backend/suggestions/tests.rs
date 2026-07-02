//! Tests for the F3.3 suggestions REST handlers.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_db::suggestions::{replace_suggestions, NewSuggestion, SuggestionState};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn post_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn sample_suggestion(id: i64, title: &str, cover: bool) -> NewSuggestion {
    NewSuggestion {
        hardcover_id: id,
        hardcover_slug: Some(format!("slug-{id}")),
        title: title.to_string(),
        author: "Read Alike".to_string(),
        list_count: id,
        cover_mime: cover.then(|| "image/jpeg".to_string()),
        cover_bytes: cover.then(|| b"\xFF\xD8\xFFjpegbytes".to_vec()),
    }
}

#[tokio::test]
async fn api_suggestions_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/suggestions"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_suggestions_ready_serves_cached_rows() {
    let (app, _state, pool) = fixture().await;
    // A configured key (settings) makes the response deterministic regardless
    // of the ambient HARDCOVER_API_KEY env var.
    omnibus_db::set_hardcover_api_key(&pool, Some("hc_test_key_value"))
        .await
        .unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[
            sample_suggestion(200, "Alpha", true),
            sample_suggestion(201, "Beta", false),
        ],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();

    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/book-1/suggestions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("\"status\":\"ready\""));
    assert!(body.contains("Alpha"));
    assert!(body.contains("https://hardcover.app/books/slug-200"));
    assert!(body.contains("/api/suggestions/book-1/0/cover"));
}

#[tokio::test]
async fn api_suggestion_cover_returns_404_on_miss() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/suggestions/nope/0/cover", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_suggestion_cover_serves_cached_bytes() {
    let (app, _state, pool) = fixture().await;
    replace_suggestions(
        &pool,
        "book-1",
        &[sample_suggestion(200, "Alpha", true)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/suggestions/book-1/0/cover", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("content-type").unwrap(), "image/jpeg");
}

#[tokio::test]
async fn api_hardcover_key_get_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/hardcover-key", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_hardcover_key_post_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_json(
            "/api/hardcover-key",
            &token,
            serde_json::json!({ "key": "hc_secret" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_hardcover_key_post_returns_422_when_key_exceeds_max_len() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "a".repeat(omnibus_shared::HARDCOVER_API_KEY_MAX_LEN + 1);
    let res = app
        .oneshot(post_json(
            "/api/hardcover-key",
            &token,
            serde_json::json!({ "key": over_limit }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_string(res).await;
    assert!(
        body.contains(&omnibus_shared::HARDCOVER_API_KEY_MAX_LEN.to_string()),
        "error body should name the cap: {body}"
    );

    // 422 must short-circuit before the write reaches the settings table.
    assert_eq!(
        omnibus_db::get_hardcover_api_key(&pool).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn api_hardcover_key_set_then_get_returns_masked_never_raw() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let raw = "hc_supersecret_jwt_value_1234";
    let set_res = app
        .clone()
        .oneshot(post_json(
            "/api/hardcover-key",
            &token,
            serde_json::json!({ "key": raw }),
        ))
        .await
        .unwrap();
    assert_eq!(set_res.status(), StatusCode::OK);
    let set_body = body_string(set_res).await;
    // The raw key must never be echoed back to the client.
    assert!(!set_body.contains(raw));
    assert!(set_body.contains("\"configured\":true"));
    assert!(set_body.contains("\"source\":\"settings\""));

    let get_res = app
        .oneshot(get_with_bearer("/api/hardcover-key", &token))
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_body = body_string(get_res).await;
    assert!(!get_body.contains(raw));
    assert!(get_body.contains("\"configured\":true"));
    // Masked preview keeps the first/last 4 chars around an ellipsis.
    assert!(get_body.contains("hc_s\u{2026}1234"));
}
