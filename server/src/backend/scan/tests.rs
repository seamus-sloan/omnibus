//! Tests for the Physical Check-In scan REST handlers. These stay network-free:
//! an exact-ISBN hit and an invalid ISBN both resolve before any provider call,
//! and the write paths don't touch the network. The full matching ladder
//! (online rungs) is covered by the `omnibus_db::scan` wiremock tests.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_db::test_support::EnvVarGuard;
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

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
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
async fn api_scan_resolve_rejects_an_oversized_isbn() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let oversized = "1".repeat(omnibus_shared::scan::ISBN_MAX_LEN + 1);
    let res = app
        .oneshot(post(
            "/api/scan/resolve",
            &token,
            serde_json::json!({ "isbn": oversized }),
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
async fn api_scan_resolve_exact_hit_returns_on_wishlist_for_wishlister() {
    let (app, _state, pool) = fixture().await;
    let uuid = seed_book_with_isbn(&pool, "Effective Java", ISBN).await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    omnibus_db::add_wishlist_entry(&pool, user.id, &uuid, WishlistSource::Manual)
        .await
        .unwrap();

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
        ScanOutcome::OnWishlist { book } => assert_eq!(book.uuid, uuid),
        other => panic!("expected OnWishlist, got {other:?}"),
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

// ── Google Books API key (admin) ─────────────────────────────────

#[tokio::test]
async fn api_google_books_key_get_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/google-books-key", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_google_books_key_post_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/google-books-key",
            &token,
            serde_json::json!({ "key": "AIzaSecret" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_google_books_key_post_returns_422_when_key_exceeds_max_len() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "a".repeat(omnibus_shared::GOOGLE_BOOKS_API_KEY_MAX_LEN + 1);
    let res = app
        .oneshot(post(
            "/api/google-books-key",
            &token,
            serde_json::json!({ "key": over_limit }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_string(res).await;
    assert!(
        body.contains(&omnibus_shared::GOOGLE_BOOKS_API_KEY_MAX_LEN.to_string()),
        "error body should name the limit: {body}"
    );

    // 422 must short-circuit before the KV write.
    assert_eq!(
        omnibus_db::get_google_books_api_key(&pool).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn api_google_books_key_set_then_get_returns_masked_never_raw() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let raw = "AIzaSySupersecretKeyValue1234";
    let set_res = app
        .clone()
        .oneshot(post(
            "/api/google-books-key",
            &token,
            serde_json::json!({ "key": raw }),
        ))
        .await
        .unwrap();
    assert_eq!(set_res.status(), StatusCode::OK);
    let set_body = body_string(set_res).await;
    // The raw key must never be echoed back to the client.
    assert!(!set_body.contains(raw));
    assert!(set_body.contains("\"configured\":true"));
    assert!(set_body.contains("\"source\":\"settings\""));

    let get_res = app
        .oneshot(get_with_bearer("/api/google-books-key", &token))
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_body = body_string(get_res).await;
    assert!(!get_body.contains(raw));
    assert!(get_body.contains("\"configured\":true"));
    // Masked preview keeps the first/last 4 chars around an ellipsis.
    assert!(get_body.contains("AIza\u{2026}1234"));
}

// ── Google Books configured flag (any authenticated user) ────────

#[tokio::test]
async fn api_google_books_configured_reflects_saved_key_for_any_authenticated_user() {
    // `google_books_key_status` falls back to `GOOGLE_BOOKS_API_KEY`, so the
    // pre-save "false" assertion below only holds with the var removed.
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/scan/google-books-configured")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "false");

    omnibus_db::set_google_books_api_key(&pool, Some("AIzaSySupersecretKeyValue1234"))
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/google-books-configured")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "true");
}

#[tokio::test]
async fn api_google_books_configured_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/google-books-configured")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn scan_error_maps_a_provider_outage_to_503_not_500() {
    use omnibus_db::{MetadataLookupError, ScanError};

    // Both providers down is an outage, not a bug: a 500 tells the reader
    // Omnibus is broken, when the honest answer is "try again later".
    let err = ScanError::Lookup(MetadataLookupError::Provider(anyhow::anyhow!(
        "google books returned an error status"
    )));
    assert_eq!(
        super::scan_error("scan_resolve", err).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn scan_error_still_maps_a_db_failure_to_500() {
    use omnibus_db::ScanError;

    let err = ScanError::Sqlx(sqlx::Error::RowNotFound);
    assert_eq!(
        super::scan_error("scan_resolve", err).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
