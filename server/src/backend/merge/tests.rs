//! Tests for the admin merge/unmerge REST endpoints: auth + admin gate,
//! the happy merge/undo round-trip, per-reader-state retargeting, and
//! the per-variant 4xx mappings.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::{MergeBooksResult, RatingUpdate, UndoMergeResult};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use omnibus_db as db;

/// POST `body` as JSON with a bearer header.
fn post_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json<T: serde::de::DeserializeOwned>(res: axum::response::Response) -> T {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Seed two distinct books (separate seed libraries so the second
/// `replace_books` doesn't sweep the first) and return their uuids.
async fn seed_source_and_target(pool: &sqlx::SqlitePool) -> (String, String) {
    let (_, source_uuid) = seed_book_with_uuid(pool, "/lib-a", "Source Book").await;
    let (_, target_uuid) = seed_book_with_uuid(pool, "/lib-b", "Target Book").await;
    (source_uuid, target_uuid)
}

async fn books_row_count(pool: &sqlx::SqlitePool, uuid: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn api_merge_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let body = serde_json::json!({ "source_uuid": "a", "target_uuid": "b" });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/books/merge")
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
async fn api_merge_rejects_a_non_admin_user() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (source_uuid, target_uuid) = seed_source_and_target(&pool).await;
    let body = serde_json::json!({ "source_uuid": source_uuid, "target_uuid": target_uuid });
    let res = app
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // The gate rejected before the transaction: both books survive.
    assert_eq!(books_row_count(&pool, &source_uuid).await, 1);
}

#[tokio::test]
async fn api_merge_merges_source_into_target_and_returns_the_undo_handle() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (source_uuid, target_uuid) = seed_source_and_target(&pool).await;
    let body = serde_json::json!({ "source_uuid": source_uuid, "target_uuid": target_uuid });
    let res = app
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let out: MergeBooksResult = body_json(res).await;
    assert_eq!(out.target_uuid, target_uuid);
    assert!(out.merge_log_id > 0);
    // The source row is gone; the target survives.
    assert_eq!(books_row_count(&pool, &source_uuid).await, 0);
    assert_eq!(books_row_count(&pool, &target_uuid).await, 1);
}

#[tokio::test]
async fn api_merge_retargets_per_reader_state_onto_the_target() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (source_uuid, target_uuid) = seed_source_and_target(&pool).await;
    // A rating keyed on the source book's uuid (`user_ratings` is in
    // `RETARGET_TABLES`) must survive the merge on the target.
    db::ratings::set_rating(
        &pool,
        admin.id,
        &RatingUpdate {
            book_uuid: source_uuid.clone(),
            stars: 4.5,
        },
    )
    .await
    .expect("seed rating");
    let body = serde_json::json!({ "source_uuid": source_uuid, "target_uuid": target_uuid });
    let res = app
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let rating = db::ratings::get_rating(&pool, admin.id, &target_uuid)
        .await
        .expect("get rating")
        .expect("rating should have been retargeted onto the surviving book");
    assert_eq!(rating.stars, 4.5);
}

#[tokio::test]
async fn api_merge_404s_for_an_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (_, target_uuid) = seed_book_with_uuid(&pool, "/lib-b", "Target Book").await;
    let body = serde_json::json!({ "source_uuid": "no-such-book", "target_uuid": target_uuid });
    let res = app
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_merge_422s_when_source_and_target_are_the_same_book() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib-a", "Only Book").await;
    let body = serde_json::json!({ "source_uuid": uuid, "target_uuid": uuid });
    let res = app
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn api_merge_500s_when_the_db_is_gone() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (source_uuid, target_uuid) = seed_source_and_target(&pool).await;
    sqlx::query("DROP TABLE merge_log")
        .execute(&pool)
        .await
        .unwrap();
    let body = serde_json::json!({ "source_uuid": source_uuid, "target_uuid": target_uuid });
    let res = app
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_undo_merge_restores_the_source_book() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (source_uuid, target_uuid) = seed_source_and_target(&pool).await;
    let body = serde_json::json!({ "source_uuid": source_uuid, "target_uuid": target_uuid });
    let res = app
        .clone()
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let merged: MergeBooksResult = body_json(res).await;

    let body = serde_json::json!({ "merge_log_id": merged.merge_log_id });
    let res = app
        .oneshot(post_json("/api/books/merge/undo", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let out: UndoMergeResult = body_json(res).await;
    assert_eq!(out.restored_uuid, source_uuid);
    // The source books row is back alongside the target.
    assert_eq!(books_row_count(&pool, &source_uuid).await, 1);
    assert_eq!(books_row_count(&pool, &target_uuid).await, 1);
}

#[tokio::test]
async fn api_undo_merge_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let body = serde_json::json!({ "merge_log_id": 1 });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/books/merge/undo")
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
async fn api_undo_merge_rejects_a_non_admin_user() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({ "merge_log_id": 1 });
    let res = app
        .oneshot(post_json("/api/books/merge/undo", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_undo_merge_404s_for_an_unknown_log_id() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let body = serde_json::json!({ "merge_log_id": 999 });
    let res = app
        .oneshot(post_json("/api/books/merge/undo", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_undo_merge_409s_when_the_merge_was_already_undone() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "root").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let (source_uuid, target_uuid) = seed_source_and_target(&pool).await;
    let body = serde_json::json!({ "source_uuid": source_uuid, "target_uuid": target_uuid });
    let res = app
        .clone()
        .oneshot(post_json("/api/books/merge", &token, body))
        .await
        .unwrap();
    let merged: MergeBooksResult = body_json(res).await;
    let body = serde_json::json!({ "merge_log_id": merged.merge_log_id });
    let res = app
        .clone()
        .oneshot(post_json("/api/books/merge/undo", &token, body.clone()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .oneshot(post_json("/api/books/merge/undo", &token, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}
