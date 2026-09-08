//! `GET`/`PUT /api/audiobooks/{uuid}/playback-rate`: auth gating, the
//! unset null, the round trip, the out-of-range rejection, the unknown
//! book, and the DB-failure paths.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use omnibus_shared::AudiobookPlaybackRateRecord;
use tower::ServiceExt;

use super::seed_one_audiobook;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn put_playback_rate(uri: &str, token: &str, playback_rate: f64) -> Request<Body> {
    let mut request = get_with_bearer(uri, token);
    *request.method_mut() = axum::http::Method::PUT;
    request.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    *request.body_mut() =
        Body::from(serde_json::json!({ "playback_rate": playback_rate }).to_string());
    request
}

#[tokio::test]
async fn api_get_playback_rate_requires_auth() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/book-a/playback-rate"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_playback_rate_returns_null_when_unset() {
    let (app, _, pool) = fixture().await;
    let uuid = seed_one_audiobook(&pool).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/playback-rate"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Option<AudiobookPlaybackRateRecord>>(&body).unwrap(),
        None
    );
}

#[tokio::test]
async fn api_put_playback_rate_round_trips_for_authenticated_user() {
    let (app, _, pool) = fixture().await;
    let uuid = seed_one_audiobook(&pool).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uri = format!("/api/audiobooks/{uuid}/playback-rate");

    let res = app
        .clone()
        .oneshot(put_playback_rate(&uri, &token, 2.25))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let saved: AudiobookPlaybackRateRecord = serde_json::from_slice(&body).unwrap();
    assert_eq!(saved.playback_rate, 2.25);

    let res = app.oneshot(get_with_bearer(&uri, &token)).await.unwrap();
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Option<AudiobookPlaybackRateRecord>>(&body).unwrap(),
        Some(saved)
    );
}

#[tokio::test]
async fn api_put_playback_rate_rejects_out_of_range_value() {
    let (app, _, pool) = fixture().await;
    let uuid = seed_one_audiobook(&pool).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_playback_rate(
            &format!("/api/audiobooks/{uuid}/playback-rate"),
            &token,
            3.5,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_playback_rate_returns_404_for_unknown_book() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_playback_rate(
            "/api/audiobooks/no-such-book/playback-rate",
            &token,
            1.5,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_playback_rate_returns_404_for_unknown_book() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/no-such-book/playback-rate",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_playback_rate_returns_500_on_db_failure() {
    let (app, _, pool) = fixture().await;
    let uuid = seed_one_audiobook(&pool).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query("DROP TABLE audiobook_playback_preferences")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/playback-rate"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_put_playback_rate_returns_500_on_db_failure() {
    let (app, _, pool) = fixture().await;
    let uuid = seed_one_audiobook(&pool).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query("DROP TABLE audiobook_playback_preferences")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(put_playback_rate(
            &format!("/api/audiobooks/{uuid}/playback-rate"),
            &token,
            1.5,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
