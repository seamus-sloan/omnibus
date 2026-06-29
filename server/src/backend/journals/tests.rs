//! Tests for public journal REST handlers.
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::JournalEntry;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn post_journal_req(token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/api/journals")
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn entries(res: axum::response::Response) -> Vec<JournalEntry> {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_post_journal_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/journals")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "book_uuid": "x", "body_md": "hi", "progress": null })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_journal_rejects_empty_body() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_journal_req(
            &token,
            serde_json::json!({ "book_uuid": "anything", "body_md": "   ", "progress": null }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_journal_returns_404_for_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_journal_req(
            &token,
            serde_json::json!({ "book_uuid": "no-such-book", "body_md": "hi", "progress": null }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_journal_create_then_list_is_public_across_users() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    let res = app
        .clone()
        .oneshot(post_journal_req(
            &alice_token,
            serde_json::json!({ "book_uuid": uuid, "body_md": "**alice** note", "progress": 30 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created = entries_one(res).await;
    assert!(created.body_html.contains("<strong>alice</strong>"));
    assert_eq!(created.progress, Some(30));

    app.clone()
        .oneshot(post_journal_req(
            &bob_token,
            serde_json::json!({ "book_uuid": uuid, "body_md": "bob note", "progress": null }),
        ))
        .await
        .unwrap();

    // Bob sees both entries (journals are public).
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/journals/book/{uuid}"),
            &bob_token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list = entries(res).await;
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].author_name, "bob", "newest first");
    assert_eq!(list[1].author_name, "alice");
}

#[tokio::test]
async fn api_patch_journal_by_non_owner_returns_404() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    let res = app
        .clone()
        .oneshot(post_journal_req(
            &alice_token,
            serde_json::json!({ "book_uuid": uuid, "body_md": "mine", "progress": null }),
        ))
        .await
        .unwrap();
    let created = entries_one(res).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/journals/{}", created.id))
                .method("PATCH")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::from(
                    serde_json::json!({ "body_md": "hijack", "progress": null }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_delete_journal_by_owner_then_list_empty() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(post_journal_req(
            &token,
            serde_json::json!({ "book_uuid": uuid, "body_md": "bye", "progress": null }),
        ))
        .await
        .unwrap();
    let created = entries_one(res).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/journals/{}", created.id))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/journals/book/{uuid}"),
            &token,
        ))
        .await
        .unwrap();
    assert!(entries(res).await.is_empty());
}

#[tokio::test]
async fn api_journal_preview_renders_sanitized_html() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/journals/preview")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::to_string("**bold** <script>x</script>").unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let html: String = serde_json::from_slice(&bytes).unwrap();
    assert!(html.contains("<strong>bold</strong>"));
    assert!(!html.contains("<script"));
}

/// Decode a single created/updated entry from a handler response.
async fn entries_one(res: axum::response::Response) -> JournalEntry {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
