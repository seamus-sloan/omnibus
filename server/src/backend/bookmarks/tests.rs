//! Integration tests for bookmark REST handlers.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::Bookmark;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_post_bookmark_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let body = serde_json::json!({ "book_uuid": "x", "position": "10.0", "title": null });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/bookmarks")
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
async fn api_post_bookmark_400_on_oversized_position() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let oversized = "a".repeat(omnibus_shared::CreateBookmark::POSITION_MAX_LEN + 1);
    let body = serde_json::json!({ "book_uuid": uuid, "position": oversized, "title": null });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/bookmarks")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("position"), "got: {msg}");
}

#[tokio::test]
async fn api_post_bookmark_404_on_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({ "book_uuid": "no-such-uuid", "position": "1.0", "title": null });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/bookmarks")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_bookmark_crud_round_trip() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Create
    let body = serde_json::json!({ "book_uuid": uuid, "position": "1234.5", "title": "A mark" });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/bookmarks")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let created: Bookmark = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created.position, "1234.5");
    assert_eq!(created.title.as_deref(), Some("A mark"));

    // List
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/bookmarks/book/{uuid}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Bookmark> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, created.id);

    // Update title
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/bookmarks/{}", created.id))
                .method("PUT")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({ "title": "renamed" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Verify update via list
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/bookmarks/book/{uuid}"),
            &token,
        ))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Bookmark> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list[0].title.as_deref(), Some("renamed"));

    // Delete
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/bookmarks/{}", created.id))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Verify deletion
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/bookmarks/book/{uuid}"),
            &token,
        ))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Bookmark> = serde_json::from_slice(&bytes).unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn api_bookmark_user_isolation() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    // Alice creates a bookmark
    let body = serde_json::json!({ "book_uuid": uuid, "position": "9.0", "title": null });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/bookmarks")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {alice_token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let created: Bookmark = serde_json::from_slice(&bytes).unwrap();

    // Bob cannot see Alice's bookmarks
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/bookmarks/book/{uuid}"),
            &bob_token,
        ))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Bookmark> = serde_json::from_slice(&bytes).unwrap();
    assert!(list.is_empty());

    // Bob cannot delete Alice's bookmark
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/bookmarks/{}", created.id))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
