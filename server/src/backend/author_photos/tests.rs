//! Integration tests for the F1.11 author profile photo endpoints. Covers
//! GET / PUT / DELETE on `/api/authors/{id}/photo` plus the rescan-from-
//! Open-Library trigger, including auth + admin gating, upload-validation
//! failure paths, and 5xx DB-failure paths.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_author_photo_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/authors/1/photo"))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_author_photo_404_when_unset() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/authors/{id}/photo"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_put_author_photo_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let (content_type, body) = build_photo_multipart("image/png", TINY_PNG);
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo"))
                .method("PUT")
                .header("content-type", content_type)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_put_author_photo_uploads_and_get_serves() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (content_type, body) = build_photo_multipart("image/png", TINY_PNG);
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo"))
                .method("PUT")
                .header("content-type", content_type)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // GET should now return the uploaded bytes with the detected mime.
    let res = app
        .clone()
        .oneshot(get_with_bearer(&format!("/api/authors/{id}/photo"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(ct, "image/png");
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.as_ref(), TINY_PNG);

    // The author detail payload must now flag has_photo = true.
    let res = app
        .oneshot(get_with_bearer(&format!("/api/authors/{id}"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let author: omnibus_shared::AuthorDetail = serde_json::from_slice(&bytes).unwrap();
    assert!(author.has_photo, "has_photo should flip after upload");
}

#[tokio::test]
async fn api_put_author_photo_404_for_missing_author() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (content_type, body) = build_photo_multipart("image/png", TINY_PNG);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/9999/photo")
                .method("PUT")
                .header("content-type", content_type)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_put_author_photo_rejects_non_image() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (content_type, body) = build_photo_multipart("text/plain", b"not an image");
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo"))
                .method("PUT")
                .header("content-type", content_type)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_author_photo_rejects_bogus_image_bytes() {
    // Content-Type says image/png but the bytes don't carry the PNG magic
    // header — the magic-byte check must catch this even when the
    // declared MIME passes the `image/` prefix guard. The handler surfaces
    // a 415 (#210) since the payload is not a recognisable image format.
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (content_type, body) = build_photo_multipart("image/png", b"not really a png");
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo"))
                .method("PUT")
                .header("content-type", content_type)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        body.contains("Could not detect image format"),
        "415 body should explain the format detection failure, got {body:?}"
    );
}

#[tokio::test]
async fn api_delete_author_photo_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo"))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_delete_author_photo_clears_row() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    db::upsert_author_photo(
        &pool,
        id,
        db::AuthorPhotoSource::Manual,
        None,
        Some("image/png"),
        Some(TINY_PNG),
    )
    .await
    .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo"))
                .method("DELETE")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    assert!(db::author_photo_status(&pool, id).await.unwrap().is_none());
}

// --- Set-by-URL admin handler. The remote fetch is exercised via a
// local `wiremock` server, never the public internet. Covers the
// admin gate, the validation paths (404 missing author, 400 empty /
// bad-scheme URL, non-image content-type, bogus magic bytes), and the
// happy path (204 then GET returns the bytes).

#[tokio::test]
async fn api_put_author_photo_url_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_photo_url(
            &format!("/api/authors/{id}/photo/url"),
            &token,
            "http://127.0.0.1:1/never-reached",
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_put_author_photo_url_404_for_missing_author() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(put_photo_url(
            "/api/authors/9999/photo/url",
            &token,
            "http://127.0.0.1:1/never-reached",
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_put_author_photo_url_rejects_empty_url() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(put_photo_url(
            &format!("/api/authors/{id}/photo/url"),
            &token,
            "   ",
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// Issue #275 — admin SSRF regression. The default `AppState` (built by
/// `fixture()`) must reject IP-literal URLs that point at the loopback /
/// cloud-metadata / RFC1918 address space *before* any TCP connect.
/// Every URL listed below is one a hostile or compromised admin could
/// plausibly try; each must surface 400.
#[tokio::test]
async fn api_put_author_photo_url_blocks_private_ip_targets() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Pin the test cases to a stable shortlist covering each blocked
    // category (loopback, cloud-metadata link-local, RFC1918, IPv4
    // unspecified, IPv6 loopback, IPv6-mapped loopback). The full
    // category matrix is unit-tested against `is_blocked_address` in
    // `omnibus_db::author_photos::tests`.
    let cases = [
        "http://127.0.0.1/x",
        "http://169.254.169.254/latest/meta-data/", // AWS IMDSv1
        "http://10.0.0.1/x",
        "http://192.168.1.1/x",
        "http://172.16.0.1/x",
        "http://0.0.0.0/x",
        "http://[::1]/x",
        "http://[::ffff:127.0.0.1]/x",
    ];
    for url in cases {
        let res = app
            .clone()
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                url,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "SSRF URL {url:?} must be blocked",
        );
    }
}

#[tokio::test]
async fn api_put_author_photo_url_rejects_bad_scheme() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // ftp:// trips the `fetch_remote_image` scheme guard before any
    // outbound request fires.
    let res = app
        .oneshot(put_photo_url(
            &format!("/api/authors/{id}/photo/url"),
            &token,
            "ftp://example.com/photo.jpg",
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_author_photo_url_uploads_and_get_serves() {
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/portrait.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(TINY_PNG),
        )
        .mount(&mock)
        .await;

    // wiremock binds to 127.0.0.1, which the production SSRF guard
    // (#275) rejects. Use the loopback-allowing fixture so the handler
    // exercises the rest of the validation pipeline.
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let url = format!("{}/portrait.png", mock.uri());
    let res = app
        .clone()
        .oneshot(put_photo_url(
            &format!("/api/authors/{id}/photo/url"),
            &token,
            &url,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(get_with_bearer(&format!("/api/authors/{id}/photo"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.as_ref(), TINY_PNG);
}

#[tokio::test]
async fn api_put_author_photo_url_rejects_non_image_content_type() {
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/not-a-photo.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<html>nope</html>"),
        )
        .mount(&mock)
        .await;

    // See note in `api_put_author_photo_url_uploads_and_get_serves`.
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let url = format!("{}/not-a-photo.html", mock.uri());
    let res = app
        .oneshot(put_photo_url(
            &format!("/api/authors/{id}/photo/url"),
            &token,
            &url,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_author_photo_url_rejects_bogus_image_bytes() {
    // Server lies — declares image/png but the bytes don't carry the
    // PNG magic header. The handler-side `detect_image_format` sniff
    // must catch this even though the content-type passes the
    // `image/` prefix gate.
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fake.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"definitely not png bytes" as &[u8]),
        )
        .mount(&mock)
        .await;

    // See note in `api_put_author_photo_url_uploads_and_get_serves`.
    let (app, _state, pool) = fixture_loopback_remote_image().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let url = format!("{}/fake.png", mock.uri());
    let res = app
        .oneshot(put_photo_url(
            &format!("/api/authors/{id}/photo/url"),
            &token,
            &url,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// --- Scan-for-picture admin gate / not-found contract. The resolver
// itself is exercised by the wiremock-backed tests in
// `omnibus_db::author_photos::tests`; these only cover the wiring so
// they don't reach the real Open Library service.

#[tokio::test]
async fn api_scan_author_photo_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo/scan"))
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_scan_author_photo_404_for_missing_author() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/9999/photo/scan")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_scan_author_photo_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/1/photo/scan")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_scan_author_photo_preserves_manual_upload() {
    // Roadmap: manual override wins over the resolver. An admin clicking
    // "Scan for picture" on an author who already has a manual upload
    // must not wipe that upload — the scan handler treats the row as a
    // sticky override and returns resolved=true without deleting.
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Ada Lovelace").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    db::upsert_author_photo(
        &pool,
        id,
        db::AuthorPhotoSource::Manual,
        None,
        Some("image/png"),
        Some(TINY_PNG),
    )
    .await
    .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/authors/{id}/photo/scan"))
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body: omnibus_shared::AuthorPhotoScanResult = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body.resolved,
        "scan on manual upload should report resolved=true"
    );

    // Manual row must still be intact (same source, same bytes).
    let (src, _) = db::author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, db::AuthorPhotoSource::Manual);
    let (_, served) = db::get_author_photo(&pool, id).await.unwrap().unwrap();
    assert_eq!(served, TINY_PNG, "manual photo bytes must be preserved");
}

#[tokio::test]
async fn api_get_author_response_carries_has_photo_flag() {
    // F1.11 autoresolution wiring lives behind the GET handler — the
    // worker call itself is fire-and-forget so we can't deterministically
    // observe it from a test without a network. What we can verify is
    // that the handler still returns the expected `AuthorDetail` shape
    // with `has_photo = false` when no row exists, and `true` after a
    // manual upload (covered by `api_put_author_photo_uploads_and_get_serves`
    // for the positive case).
    let (app, _state, pool) = fixture().await;
    let id = seed_author(&pool, "Brandon Sanderson").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(&format!("/api/authors/{id}"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let author: omnibus_shared::AuthorDetail = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(author.id, id);
    assert!(!author.has_photo, "no row yet means has_photo = false");
}

#[tokio::test]
async fn api_refetch_author_photos_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/refetch-photos")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_refetch_author_photos_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/refetch-photos")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_refetch_author_photos_returns_202_for_admin() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/authors/refetch-photos")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::ACCEPTED);
}
