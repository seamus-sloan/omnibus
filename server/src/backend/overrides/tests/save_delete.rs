//! `POST`/`DELETE /api/ebooks/:uuid/overrides` — save and revert a
//! metadata-override layer.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use super::super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_post_overrides_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let body = serde_json::json!({ "title": "Edited" });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/ebooks/1/overrides")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_overrides_requires_edit_permission() {
    // A plain user from `create_user` has `can_edit = false`, so the
    // handler's per-route check must reject them with 403 before the
    // override row is touched.
    let (app, _state, pool) = fixture().await;
    let id = seed_book(&pool, "/lib", "Original").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let body = serde_json::json!({ "title": "Edited" });
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{id}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // No override row should have been written.
    let books = db::list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    assert!(
        db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "403 path must not persist any override"
    );
}

#[tokio::test]
async fn api_post_overrides_rejects_over_length_title_with_400() {
    // An admin submitting a title that exceeds TITLE_MAX_LEN must receive a
    // 400 and no override row must be written. This exercises the
    // `overrides.validate()` guard in the handler.
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit_title = "x".repeat(omnibus_shared::MetadataOverrides::TITLE_MAX_LEN + 1);
    let body = serde_json::json!({ "title": over_limit_title });
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        body.contains("title"),
        "400 body should name the offending field, got: {body:?}"
    );

    // No override row must have been written.
    assert!(
        db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "validation failure must not persist any override row"
    );
}

#[tokio::test]
async fn api_post_overrides_saves_and_returns_merged_book() {
    // Admin (which carries `can_edit = true` via test_support::create_admin)
    // POSTs an override. The handler must persist it, return the merged
    // book, and flip `has_override` on the response.
    let (app, _state, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "title": "Edited Title",
        "publisher": "Edited Publisher",
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(book.id, id);
    assert_eq!(book.title.as_deref(), Some("Edited Title"));
    assert_eq!(book.publisher.as_deref(), Some("Edited Publisher"));
    assert!(
        book.has_override,
        "merged book should advertise has_override = true"
    );

    // The override row must reflect the saved fields.
    let books = db::list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let (saved, has_cover) = db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .expect("override row should exist after POST");
    assert_eq!(saved.title.as_deref(), Some("Edited Title"));
    assert_eq!(saved.publisher.as_deref(), Some("Edited Publisher"));
    assert!(!has_cover, "text-only edit must not set has_cover_override");
}

#[tokio::test]
async fn api_post_overrides_round_trips_isbn10_and_print_pages() {
    // AC1: genres, print_pages, and isbn10 round-trip through the same POST
    // /api/ebooks/{uuid}/overrides path and read back on the merged book.
    let (app, _state, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "isbn10": "0134685997",
        "print_pages": 412,
        "genres": ["Science Fiction"],
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(book.id, id);
    assert_eq!(book.isbn10.as_deref(), Some("0134685997"));
    assert_eq!(book.print_pages, Some(412));
    assert_eq!(book.genres, vec!["Science Fiction".to_string()]);

    let (saved, _) = db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .expect("override row should exist after POST");
    assert_eq!(saved.isbn10.as_deref(), Some("0134685997"));
    assert_eq!(saved.print_pages, Some(412));
}

#[tokio::test]
async fn api_post_overrides_rejects_malformed_isbn10_and_out_of_range_print_pages_with_400() {
    // AC3: a malformed ISBN-10 and an out-of-range print page count each
    // return 400 with no override row written.
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({ "isbn10": "not-an-isbn" });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let over_max = omnibus_shared::MetadataOverrides::PRINT_PAGES_MAX + 1;
    let body = serde_json::json!({ "print_pages": over_max });
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    assert!(
        db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "neither validation failure should have persisted an override row"
    );
}

#[tokio::test]
async fn api_delete_overrides_reverts() {
    // Persist an override via the same REST path the client uses, then
    // delete it and assert the response reflects the canonical scanned
    // values (no `Option` overrides applied).
    let (app, _state, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let post_body = serde_json::json!({ "title": "Edited" });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(post_body.to_string()))
                .unwrap(),
        )
        .await
        .expect("POST should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/overrides"))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("DELETE should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(book.id, id);
    assert_eq!(
        book.title.as_deref(),
        Some("Original"),
        "delete must revert to the scanned title"
    );
    assert!(
        !book.has_override,
        "delete must clear the has_override flag on the merged book"
    );

    // And the override row must be gone from the DB.
    let books = db::list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    assert!(
        db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "delete must drop the metadata_overrides row"
    );
}
