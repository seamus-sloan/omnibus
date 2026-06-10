//! Tests for progress-sync REST handlers.
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::ProgressRecord;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_post_progress_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let body = serde_json::json!({
        "book_uuid": "x",
        "format": "epub",
        "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress")
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
async fn api_post_progress_rejects_missing_cfi() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({
        "book_uuid": "anything",
        "format": "epub",
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_progress_rejects_cross_format_field() {
    // `{format:"epub", audio_position_seconds:…}` must 400 at the
    // boundary so the migration's CHECK doesn't surface as a 500
    // (issue: copilot review on #300).
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({
        "book_uuid": "anything",
        "format": "epub",
        "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
        "audio_position_seconds": 12.0,
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_progress_404_on_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({
        "book_uuid": "no-such-uuid",
        "format": "epub",
        "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress")
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
async fn api_progress_round_trip_last_write_wins() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // First write.
    let body = serde_json::json!({
        "book_uuid": uuid,
        "format": "epub",
        "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/progress")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Second write — overwrites.
    let body = serde_json::json!({
        "book_uuid": uuid,
        "format": "epub",
        "epub_cfi": "epubcfi(/6/12!/4/8/3:7)",
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/progress")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Read back — last write wins.
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/progress/{uuid}?format=epub"))
                .method("GET")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<ProgressRecord> = serde_json::from_slice(&bytes).unwrap();
    let rec = rec.unwrap();
    assert_eq!(rec.epub_cfi.as_deref(), Some("epubcfi(/6/12!/4/8/3:7)"));
    assert_eq!(rec.format, ProgressFormat::Epub);
}

#[tokio::test]
async fn api_post_sessions_records_count() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!([
        {
            "book_uuid": uuid,
            "format": "epub",
            "started_at": 100,
            "ended_at": 460,
            "progress_units": 360,
        }
    ]);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress/sessions")
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
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["recorded"], 1);
}

#[tokio::test]
async fn api_post_sessions_reports_only_inserted_when_some_skipped() {
    // One real uuid + one unknown uuid. Handler must report
    // `recorded: 1`, not 2 — silently-skipped reports are tracked
    // separately so the mobile client can detect data loss (issue:
    // copilot review on #300).
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!([
        { "book_uuid": uuid,             "format": "epub", "started_at": 100, "ended_at": 460, "progress_units": 360 },
        { "book_uuid": "no-such-uuid",   "format": "epub", "started_at": 100, "ended_at": 460, "progress_units": 360 },
    ]);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress/sessions")
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
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["recorded"], 1);
}

#[tokio::test]
async fn api_post_sessions_rejects_inverted_time_range() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!([
        { "book_uuid": uuid, "format": "epub", "started_at": 500, "ended_at": 200, "progress_units": 0 }
    ]);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress/sessions")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_sessions_batch_of_two_records_both() {
    // Two valid reports in one POST must both land atomically; `recorded`
    // must reflect the full count (not a partial commit).
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!([
        { "book_uuid": uuid, "format": "epub",  "started_at": 100, "ended_at": 460, "progress_units": 360 },
        { "book_uuid": uuid, "format": "audio", "started_at": 500, "ended_at": 900, "progress_units": 400 },
    ]);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress/sessions")
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
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["recorded"], 2);
}
