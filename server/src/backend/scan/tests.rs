//! Tests for the Physical Check-In scan REST handlers. These stay network-free:
//! an exact-ISBN hit and an invalid ISBN both resolve before any provider call,
//! and the write paths don't touch the network. The full matching ladder
//! (online rungs) is covered by the `omnibus_db::scan` wiremock tests.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::{BookRef, ScanOutcome};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

const ISBN: &str = "9780134685991";

/// A well-formed `ExternalBookMeta` JSON body, as `AddPhysicalOnlyRequest` and
/// `WishlistAddRequest` embed it.
fn external_meta_json(title: &str, isbn13: &str) -> serde_json::Value {
    serde_json::json!({
        "isbn13": isbn13,
        "title": title,
        "authors": ["Jane Doe"],
        "year": null,
        "pages": null,
        "publisher": null,
        "description": null,
        "cover_url": null,
        "source": "open_library",
    })
}

fn post(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn seed_book_with_isbn(pool: &sqlx::SqlitePool, title: &str, isbn: &str) -> String {
    let (id, uuid) = seed_book_with_uuid(pool, "/lib", title).await;
    sqlx::query("INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?1, 'ISBN', ?2)")
        .bind(id)
        .bind(isbn)
        .execute(pool)
        .await
        .unwrap();
    uuid
}

async fn json_body<T: serde::de::DeserializeOwned>(res: axum::response::Response) -> T {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_scan_resolve_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/resolve")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "isbn": ISBN }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

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
async fn api_scan_resolve_rejects_invalid_isbn() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": "12345" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_scan_resolve_exact_hit_returns_in_library_unowned() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": ISBN }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    match json_body::<ScanOutcome>(res).await {
        ScanOutcome::InLibraryUnowned { book } => assert_eq!(book.uuid, uuid),
        other => panic!("expected InLibraryUnowned, got {other:?}"),
    }
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
