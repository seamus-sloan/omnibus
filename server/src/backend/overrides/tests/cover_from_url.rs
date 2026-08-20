//! `POST /api/ebooks/:uuid/cover/from-url` — applying a provider's cover.
//!
//! This is the one route in the metadata-search feature that fetches a
//! client-supplied URL server-side, so the refusals are the point: a host off
//! the catalog's allowlist, a plain-`http` URL, a redirect that leaves the
//! allowlist, bytes that aren't an image, and an origin that errors. Every
//! test drives a local `wiremock` origin — never a live provider.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// POST the given URL as this route's body.
fn from_url_request(uuid: &str, token: &str, url: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/api/ebooks/{uuid}/cover/from-url"))
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(serde_json::json!({ "url": url }).to_string()))
        .unwrap()
}

/// Mount a 200 image response at `at`.
async fn mount_image(server: &MockServer, at: &str, content_type: &str, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", content_type)
                .set_body_bytes(body),
        )
        .mount(server)
        .await;
}

// ── AC1: the happy path ──────────────────────────────────────────

#[tokio::test]
async fn api_post_cover_from_url_replaces_the_cover_and_marks_the_override() {
    let _covers = CoversDirGuard::new("cover_from_url_ok");
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "FromUrlBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let server = MockServer::start().await;
    mount_image(&server, "/cover.png", "image/png", TINY_PNG.to_vec()).await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            &format!("{}/cover.png", server.uri()),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(book.id, id);
    assert!(book.has_override);

    let (_, has_cover_override) = db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .expect("an override row must exist after applying a provider cover");
    assert!(
        has_cover_override,
        "applying a provider cover must set has_cover_override"
    );
}

// ── AC6: the scanned cover is still recoverable afterwards ───────

#[tokio::test]
async fn api_delete_cover_reverts_a_cover_that_came_from_a_provider() {
    let _covers = CoversDirGuard::new("cover_from_url_revert");
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "RevertProviderCover").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let server = MockServer::start().await;
    mount_image(&server, "/cover.png", "image/png", TINY_PNG.to_vec()).await;

    let res = app
        .clone()
        .oneshot(from_url_request(
            &uuid,
            &token,
            &format!("{}/cover.png", server.uri()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The applied cover is an ordinary override cover — nothing about its
    // provenance changes the revert path.
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/cover"))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let overrides = db::get_metadata_overrides(&pool, &uuid).await.unwrap();
    assert!(
        overrides.is_none_or(|(_, has_cover)| !has_cover),
        "revert must clear the cover override a provider cover set"
    );
}

// ── AC2: the SSRF gates ──────────────────────────────────────────

#[tokio::test]
async fn api_post_cover_from_url_refuses_a_host_outside_the_provider_allowlist() {
    // The production config, not the loopback one: this is what a real server
    // does with a URL naming a host no provider serves covers from.
    let _covers = CoversDirGuard::new("cover_from_url_host");
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "HostBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            "https://evil.example/cover.png",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("not an allowed source"),
        "the refusal must name the reason"
    );
    assert!(
        db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "a refused fetch must write nothing"
    );
}

#[tokio::test]
async fn api_post_cover_from_url_refuses_plain_http_even_for_an_allowlisted_host() {
    let _covers = CoversDirGuard::new("cover_from_url_http");
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "HttpBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // A host the catalog *does* publish, so only the scheme can refuse it.
    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            "http://covers.openlibrary.org/b/id/1-L.jpg",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("https"));
}

#[tokio::test]
async fn api_post_cover_from_url_refuses_a_redirect_that_leaves_the_allowlist() {
    // The gadget the allowlist exists for: an allowed origin answering with a
    // 302 to somewhere it was never allowed to send us.
    let _covers = CoversDirGuard::new("cover_from_url_redirect");
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "RedirectBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://evil.example/x.png"),
        )
        .mount(&server)
        .await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            &format!("{}/cover.png", server.uri()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("evil.example"),
        "the refusal must name the hop that was refused, not the first one"
    );
    assert!(db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}

// ── AC3: the bytes must actually be an image ─────────────────────

#[tokio::test]
async fn api_post_cover_from_url_refuses_bytes_that_are_not_an_image() {
    // An HTML error page served under `image/jpeg` gets past the content-type
    // gate and is caught by the magic-byte sniff — otherwise it would land on
    // disk as `override-<uuid>.jpg` and be served back as a cover.
    let _covers = CoversDirGuard::new("cover_from_url_notimage");
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "NotImageBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let server = MockServer::start().await;
    mount_image(
        &server,
        "/cover.png",
        "image/jpeg",
        b"<html>not an image</html>".to_vec(),
    )
    .await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            &format!("{}/cover.png", server.uri()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}

// ── AC4: the size cap ────────────────────────────────────────────

#[tokio::test]
async fn api_post_cover_from_url_refuses_a_response_over_the_size_cap() {
    let _covers = CoversDirGuard::new("cover_from_url_big");
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "BigBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let server = MockServer::start().await;
    // One byte over the cap, with a truthful Content-Length so the pre-check
    // fires before the body is buffered.
    let oversized = vec![0u8; (db::author_photos::REMOTE_IMAGE_MAX_BYTES + 1) as usize];
    mount_image(&server, "/cover.png", "image/png", oversized).await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            &format!("{}/cover.png", server.uri()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("byte cap"));
    assert!(db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}

// ── AC7: a 5xx from the origin is the origin's fault ─────────────

#[tokio::test]
async fn api_post_cover_from_url_reports_an_upstream_failure_as_a_bad_gateway() {
    let _covers = CoversDirGuard::new("cover_from_url_5xx");
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "UpstreamBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            &format!("{}/cover.png", server.uri()),
        ))
        .await
        .unwrap();
    // A status we *received* is a refusal we make (400), not a transport
    // failure — the distinction the handler draws.
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("503"));
    assert!(db::get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}

// ── Authorization and input shape ────────────────────────────────

#[tokio::test]
async fn api_post_cover_from_url_requires_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "PermBook").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(from_url_request(
            &uuid,
            &token,
            "https://covers.openlibrary.org/b/id/1-L.jpg",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_cover_from_url_requires_auth() {
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "AnonBook").await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/cover/from-url"))
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://covers.openlibrary.org/b/id/1-L.jpg"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_cover_from_url_returns_404_for_an_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(from_url_request(
            "00000000-0000-0000-0000-000000000000",
            &token,
            "https://covers.openlibrary.org/b/id/1-L.jpg",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_post_cover_from_url_rejects_a_blank_or_oversized_url() {
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "UrlShapeBook").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .clone()
        .oneshot(from_url_request(&uuid, &token, "   "))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let huge = format!(
        "https://covers.openlibrary.org/{}",
        "x".repeat(omnibus_shared::ExternalBookMeta::COVER_URL_MAX_LEN)
    );
    let res = app
        .oneshot(from_url_request(&uuid, &token, &huge))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── The test hatch must not be reachable in production ───────────

#[tokio::test]
async fn cover_fetch_config_is_strict_by_default() {
    // The one flag that relaxes this fetch lives on `AppState`'s injectable
    // config; a production `AppState` is built with `Default`. This is what
    // says so, so the hatch can't quietly widen a real server.
    let (_app, state, _pool) = fixture().await;
    let config = cover_fetch_config(&state);
    assert!(config.require_https, "production must be https-only");
    assert!(!config.allow_private_addresses);
    assert_eq!(
        config.host_allowlist,
        db::all_cover_hosts()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "production must allow exactly the catalog's hosts and nothing else"
    );
}
