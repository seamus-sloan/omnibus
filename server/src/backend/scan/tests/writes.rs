//! The write routes — check-in, add-physical-only and wishlist-add: auth,
//! the unknown book, the copy and wishlist entries recorded (by uuid, by
//! meta, uuid preferred when both are given), the oversized-field and
//! missing-target 400s, and the DB-failure path.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::BookRef;
use tower::ServiceExt;

use super::{body_string, external_meta_json, json_body, post, seed_book_with_isbn, ISBN};
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_scan_check_in_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/check-in")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "book_uuid": "x" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_check_in_returns_404_for_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/scan/check-in",
            &token,
            serde_json::json!({ "book_uuid": "nope" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_scan_check_in_records_a_copy() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/check-in",
            &token,
            serde_json::json!({ "book_uuid": uuid, "isbn": ISBN }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: BookRef = json_body(res).await;
    assert_eq!(body.book_uuid, uuid);
    assert_eq!(
        omnibus_db::list_physical_copies(&pool, &uuid)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn api_scan_add_physical_only_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/physical-only")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "meta": external_meta_json("Some Book", ISBN) })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_wishlist_add_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/wishlist")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "book_uuid": "x", "source": "scan" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_add_physical_only_creates_a_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/physical-only",
            &token,
            serde_json::json!({ "meta": external_meta_json("The Pragmatic Programmer", ISBN) }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: BookRef = json_body(res).await;

    let (title, path): (String, String) =
        sqlx::query_as("SELECT title, path FROM books WHERE uuid = ?1")
            .bind(&body.book_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "The Pragmatic Programmer");
    assert_eq!(path, "", "a physical-only book is fileless");
    assert_eq!(
        omnibus_db::list_physical_copies(&pool, &body.book_uuid)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn api_scan_add_physical_only_returns_500_on_db_failure() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query("DROP TABLE physical_copies")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(post(
            "/api/scan/physical-only",
            &token,
            serde_json::json!({ "meta": external_meta_json("Some Book", ISBN) }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_scan_wishlist_add_records_entry_by_uuid() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/wishlist",
            &token,
            serde_json::json!({ "book_uuid": uuid, "source": "scan" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: BookRef = json_body(res).await;
    assert_eq!(body.book_uuid, uuid);

    let entries = omnibus_db::list_wishlist(&pool, user.id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].book_uuid, uuid);
    assert_eq!(entries[0].source, WishlistSource::Scan);
}

#[tokio::test]
async fn api_scan_wishlist_add_records_entry_by_meta() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/wishlist",
            &token,
            serde_json::json!({
                "meta": external_meta_json("Wishlisted Book", ISBN),
                "source": "detail",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: BookRef = json_body(res).await;

    let (title, path): (String, String) =
        sqlx::query_as("SELECT title, path FROM books WHERE uuid = ?1")
            .bind(&body.book_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "Wishlisted Book");
    assert_eq!(path, "", "a wishlisted-by-meta book is fileless");

    let entries = omnibus_db::list_wishlist(&pool, user.id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].book_uuid, body.book_uuid);
    assert_eq!(entries[0].source, WishlistSource::Detail);
}

#[tokio::test]
async fn api_scan_wishlist_add_prefers_book_uuid_over_meta_when_both_given() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let books_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(post(
            "/api/scan/wishlist",
            &token,
            serde_json::json!({
                "book_uuid": uuid,
                "meta": external_meta_json("Should Be Ignored", ISBN),
                "source": "scan",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: BookRef = json_body(res).await;
    assert_eq!(body.book_uuid, uuid, "book_uuid should win over meta");

    let books_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        books_after, books_before,
        "meta must not create a new book when book_uuid is present"
    );

    let entries = omnibus_db::list_wishlist(&pool, user.id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].book_uuid, uuid);
}

#[tokio::test]
async fn api_scan_check_in_returns_400_for_an_oversized_note() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/check-in",
            &token,
            serde_json::json!({
                "book_uuid": uuid,
                "note": "x".repeat(omnibus_shared::scan::NOTE_MAX_LEN + 1),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res).await;
    assert!(body.contains("note"), "got: {body}");

    // The 400 must short-circuit before any copy is recorded.
    assert_eq!(
        omnibus_db::list_physical_copies(&pool, &uuid)
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn api_scan_add_physical_only_returns_400_for_an_oversized_meta_title() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized_title =
        "x".repeat(omnibus_shared::metadata_lookup::ExternalBookMeta::TITLE_MAX_LEN + 1);
    let res = app
        .oneshot(post(
            "/api/scan/physical-only",
            &token,
            serde_json::json!({ "meta": external_meta_json(&oversized_title, ISBN) }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res).await;
    assert!(body.contains("title"), "got: {body}");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "an invalid request must not create a book");
}

#[tokio::test]
async fn api_scan_wishlist_add_returns_400_for_an_oversized_book_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/wishlist",
            &token,
            serde_json::json!({
                "book_uuid": "u".repeat(omnibus_shared::BOOK_UUID_MAX_LEN + 1),
                "source": "scan",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_string(res).await;
    assert!(body.contains("bytes"), "got: {body}");
}

#[tokio::test]
async fn api_scan_wishlist_add_returns_400_when_neither_uuid_nor_meta_given() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/wishlist",
            &token,
            serde_json::json!({ "source": "manual" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
