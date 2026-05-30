// ---------------------------------------------------------------------------
// F1.11 Author profile photos.
// ---------------------------------------------------------------------------

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, worker::Task};
use omnibus_shared::detect_image_format;
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::{AdminUser, AuthUser};

/// Serve a cached author profile photo. Returns 404 when no photo is cached
/// (including `'letter'` negative-cache markers) — the frontend keeps the
/// letter avatar in that case. On a miss, enqueues a background resolution
/// task so a subsequent page view can render the resolved photo.
pub(super) async fn get_author_photo(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_author_photo(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Match `/api/covers/:id` cache semantics.
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => {
            // Only queue resolution if no row exists yet — a `letter` marker
            // is a sticky miss until an admin DELETEs it. `author_photo_status`
            // returns the row regardless of source.
            if let Ok(None) = db::author_photo_status(&state.pool, id).await {
                state
                    .worker
                    .post(Task::ResolveAuthorPhoto { author_id: id });
            }
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => internal("read author photo", error),
    }
}

/// Admin upload of a manual author profile photo. Multipart with one
/// `photo` field. Mirrors the `/api/ebooks/{id}/cover` validation
/// pipeline: 10 MiB cap, magic-byte sniff, SVG rejection.
pub(super) async fn put_author_photo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    // Confirm the author exists before reading multipart so a malformed
    // upload to a missing id fails fast with 404 (not 400).
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }

    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "photo" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "photo must be an image",
                    )
                        .into_response();
                }
                if content_type.contains("svg") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "SVG photos are not accepted",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "photo must be under 10 MB",
                            )
                                .into_response();
                        }
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            Some(mime) => break (mime, b),
                            None => {
                                return (
                                    axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                    "Could not detect image format",
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => return internal("read photo field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'photo' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    if let Err(e) = db::upsert_author_photo(
        &state.pool,
        id,
        db::AuthorPhotoSource::Manual,
        None,
        Some(&mime),
        Some(&bytes),
    )
    .await
    {
        return internal("upsert_author_photo", e);
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

/// Admin: drop the cached photo so the next page view re-queues resolution.
pub(super) async fn delete_author_photo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::delete_author_photo(&state.pool, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal("delete_author_photo", e),
    }
}

/// Admin: synchronously run the Open Library cascade for an author. Clears
/// any sticky `letter` negative-cache marker so the resolver re-queries Open
/// Library, then awaits the resolver inline and returns
/// `{ "resolved": bool }`. `resolved=false` means Open Library had nothing
/// (a `letter` marker is now in place to skip future autoresolution).
///
/// Manual uploads are treated as overrides: a `source = 'manual'` row is
/// preserved (the F1.11 roadmap explicitly calls this out — "skips if a
/// manual override exists"). Scan returns `resolved=true` in that case
/// without touching the row, so admins can't accidentally wipe a manual
/// upload by clicking the button.
pub(super) async fn post_author_photo_scan(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    // Verify the author exists first so a typo on the id gets a 404 instead
    // of a successful no-op scan.
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }
    // Manual uploads win — don't delete or overwrite. Treat scan as a no-op
    // and report resolved=true so the UI keeps the existing photo.
    match db::author_photo_status(&state.pool, id).await {
        Ok(Some((db::AuthorPhotoSource::Manual, _))) => {
            return Json(omnibus_shared::AuthorPhotoScanResult { resolved: true }).into_response();
        }
        Ok(_) => {}
        Err(e) => return internal("author_photo_status (pre-scan)", e),
    }
    if let Err(e) = db::delete_author_photo(&state.pool, id).await {
        return internal("delete_author_photo (pre-scan)", e);
    }
    if let Err(e) = db::author_photos::resolve(&state.pool, id).await {
        return internal("author_photos::resolve", e);
    }
    let resolved = match db::get_author_photo(&state.pool, id).await {
        Ok(opt) => opt.is_some(),
        Err(e) => return internal("get_author_photo (post-scan)", e),
    };
    Json(omnibus_shared::AuthorPhotoScanResult { resolved }).into_response()
}

/// JSON body for [`put_author_photo_url`]. Kept inline because the shape is
/// trivial and not shared with any other call site (the RPC server function
/// passes the URL as a positional arg).
#[derive(Debug, Deserialize)]
pub(super) struct AuthorPhotoUrlBody {
    url: String,
}

/// Admin: persist an author photo by URL. Server-side fetches the URL,
/// validates content-type/size/magic-bytes, and stores it as a `manual`
/// row — the same source as a multipart upload, so it wins over Open
/// Library resolution and survives a "Scan for picture" click.
pub(super) async fn put_author_photo_url(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<AuthorPhotoUrlBody>,
) -> Response {
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }

    let url = body.url.trim();
    if url.is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, "url is required").into_response();
    }

    let (advertised_mime, bytes) =
        match db::author_photos::fetch_remote_image_with(url, state.remote_image_config()).await {
            Ok(pair) => pair,
            Err(db::author_photos::FetchRemoteImageError::Http(e)) => {
                return internal("fetch remote image", e);
            }
            Err(e) => {
                // All other variants are validation errors (bad scheme, blocked
                // SSRF target, non-image content-type, too-large, …) — map to
                // 400 with the user-facing message from `#[error]`.
                return (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response();
            }
        };

    // Magic-byte sniff — don't trust the remote Content-Type header.
    let mime = match detect_image_format(&bytes) {
        Some(m) => m,
        None => {
            tracing::warn!(
                advertised_mime,
                "remote URL returned image content-type but bytes are not a recognized image"
            );
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "file at URL does not appear to be a valid image",
            )
                .into_response();
        }
    };

    if let Err(e) = db::upsert_author_photo(
        &state.pool,
        id,
        db::AuthorPhotoSource::Manual,
        Some(url),
        Some(&mime),
        Some(&bytes),
    )
    .await
    {
        return internal("upsert_author_photo (url)", e);
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    // ---------------------------------------------------------------------
    // F1.11 Author profile photos.
    // ---------------------------------------------------------------------

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
}
