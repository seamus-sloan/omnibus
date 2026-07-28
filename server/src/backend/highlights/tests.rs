//! Integration tests for highlight annotation REST handlers.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::Highlight;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_post_highlight_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let body = serde_json::json!({
        "book_uuid": "x",
        "epub_cfi_range": "epubcfi(/6/4)",
        "color": "amber",
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
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
async fn api_post_highlight_400_on_oversized_cfi() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let oversized = "a".repeat(omnibus_shared::CreateHighlight::EPUB_CFI_RANGE_MAX_LEN + 1);
    let body = serde_json::json!({
        "book_uuid": uuid,
        "epub_cfi_range": oversized,
        "color": "amber",
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
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
    assert!(msg.contains("epub_cfi_range"), "got: {msg}");
}

#[tokio::test]
async fn api_patch_highlight_note_400_on_oversized_note() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Create a highlight to patch.
    let body = serde_json::json!({
        "book_uuid": uuid,
        "epub_cfi_range": "epubcfi(/6/4)",
        "color": "amber",
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
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
    let created: Highlight = serde_json::from_slice(&bytes).unwrap();

    let oversized = "n".repeat(omnibus_shared::UpdateHighlightNote::NOTE_MAX_LEN + 1);
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/{}/note", created.id))
                .method("PATCH")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({ "note": oversized }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let msg = String::from_utf8_lossy(&bytes);
    assert!(msg.contains("note"), "got: {msg}");
}

#[tokio::test]
async fn api_post_highlight_404_on_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({
        "book_uuid": "no-such-uuid",
        "epub_cfi_range": "epubcfi(/6/4)",
        "color": "amber",
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
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
async fn api_highlight_crud_round_trip() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Create
    let body = serde_json::json!({
        "book_uuid": uuid,
        "epub_cfi_range": "epubcfi(/6/4!/4/2,/1:0,/1:100)",
        "color": "blue",
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
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
    let created: Highlight = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created.color, omnibus_shared::HighlightColor::Blue);
    assert_eq!(
        created.epub_cfi_range.as_deref(),
        Some("epubcfi(/6/4!/4/2,/1:0,/1:100)")
    );

    // List
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/book/{uuid}"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Highlight> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, created.id);

    // Update color
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/{}/color", created.id))
                .method("PATCH")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({ "color": "rose" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Update note
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/{}/note", created.id))
                .method("PATCH")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({ "note": "key passage" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Verify updates via list
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/book/{uuid}"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Highlight> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list[0].color, omnibus_shared::HighlightColor::Rose);
    assert_eq!(list[0].note.as_deref(), Some("key passage"));

    // Delete
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/{}", created.id))
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
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/book/{uuid}"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Highlight> = serde_json::from_slice(&bytes).unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn api_highlight_user_isolation() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    // Alice creates a highlight
    let body = serde_json::json!({
        "book_uuid": uuid,
        "epub_cfi_range": "epubcfi(/6/4)",
        "color": "green",
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
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
    let created: Highlight = serde_json::from_slice(&bytes).unwrap();

    // Bob cannot see Alice's highlights
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/book/{uuid}"))
                .header(AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Highlight> = serde_json::from_slice(&bytes).unwrap();
    assert!(list.is_empty());

    // Bob cannot delete Alice's highlight
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/{}", created.id))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_delete_highlight_accepts_client_minted_id() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let client_id = "b2d7e510-0000-4000-8000-00000000000a";

    let create = serde_json::json!({
        "book_uuid": uuid,
        "epub_cfi_range": "epubcfi(/6/4!/4/2)",
        "color": "amber",
        "client_id": client_id,
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/highlights")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(create.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // This is the offline create-then-delete replay: the queued DELETE names
    // the uuid the device minted, never the row id the server assigned.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/{client_id}"))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/highlights/book/{uuid}"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let list: Vec<Highlight> = serde_json::from_slice(&bytes).unwrap();
    assert!(list.is_empty(), "delete by client id must remove the row");
}

#[tokio::test]
async fn api_patch_highlight_color_404s_for_unknown_client_id() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({ "color": "green" });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/highlights/never-minted-handle/color")
                .method("PATCH")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
