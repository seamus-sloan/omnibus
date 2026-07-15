//! Tests for star-rating REST handlers.
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::{AttributedRating, RatingRecord};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn post_rating_req(token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/api/ratings")
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn api_post_rating_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/ratings")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "book_uuid": "x", "stars": 3 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_rating_rejects_out_of_range_stars() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_rating_req(
            &token,
            serde_json::json!({ "book_uuid": "anything", "stars": 6 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_rating_returns_404_for_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_rating_req(
            &token,
            serde_json::json!({ "book_uuid": "no-such-book", "stars": 3 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_rating_round_trip_last_write_wins() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(post_rating_req(
            &token,
            serde_json::json!({ "book_uuid": uuid, "stars": 2 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(post_rating_req(
            &token,
            serde_json::json!({ "book_uuid": uuid, "stars": 4.5 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .oneshot(get_with_bearer(&format!("/api/ratings/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<RatingRecord> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rec.unwrap().stars, 4.5);
}

#[tokio::test]
async fn api_post_rating_rejects_non_half_step() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_rating_req(
            &token,
            serde_json::json!({ "book_uuid": "anything", "stars": 4.3 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_rating_returns_null_when_unrated() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(&format!("/api/ratings/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<RatingRecord> = serde_json::from_slice(&bytes).unwrap();
    assert!(rec.is_none());
}

#[tokio::test]
async fn api_delete_rating_clears_then_get_is_null() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    app.clone()
        .oneshot(post_rating_req(
            &token,
            serde_json::json!({ "book_uuid": uuid, "stars": 4 }),
        ))
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/ratings/{uuid}"))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(get_with_bearer(&format!("/api/ratings/{uuid}"), &token))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<RatingRecord> = serde_json::from_slice(&bytes).unwrap();
    assert!(rec.is_none(), "rating cleared");
}

#[tokio::test]
async fn api_rating_is_isolated_per_user() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    app.clone()
        .oneshot(post_rating_req(
            &alice_token,
            serde_json::json!({ "book_uuid": uuid, "stars": 1 }),
        ))
        .await
        .unwrap();

    // Bob has no rating for the same book.
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ratings/{uuid}"), &bob_token))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<RatingRecord> = serde_json::from_slice(&bytes).unwrap();
    assert!(rec.is_none(), "bob sees no rating from alice");
}

#[tokio::test]
async fn api_other_ratings_requires_auth() {
    let (app, _state, _pool) = fixture().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/ratings/others/book")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_other_ratings_returns_other_users_and_excludes_viewer() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    app.clone()
        .oneshot(post_rating_req(
            &alice_token,
            serde_json::json!({ "book_uuid": uuid, "stars": 2 }),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(post_rating_req(
            &bob_token,
            serde_json::json!({ "book_uuid": uuid, "stars": 4.5 }),
        ))
        .await
        .unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ratings/others/{uuid}"),
            &alice_token,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let ratings: Vec<AttributedRating> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ratings.len(), 1);
    assert_eq!(ratings[0].user_id, bob.id);
    assert_eq!(ratings[0].username, "bob");
    assert_eq!(ratings[0].stars, 4.5);
}

#[tokio::test]
async fn api_other_ratings_returns_500_on_db_failure() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, alice.id).await;
    sqlx::query("DROP TABLE user_ratings")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ratings/others/{uuid}"),
            &token,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
