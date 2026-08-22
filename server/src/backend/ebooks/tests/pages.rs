//! `GET /api/ebooks/{uuid}/pages/{page}`: the comic-page route's conditional
//! headers, query-token auth, out-of-range and wrong-format 404s, and its
//! DB- and archive-failure 500s.

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;
use super::{seed_cbz_on_disk, seed_epub_on_disk};

#[tokio::test]
async fn api_get_ebook_page_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/pages/0"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_page_returns_200_with_etag_and_media_vary() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    // Index 0 is p1.jpg by natural sort, not p2.png by zip entry order.
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/pages/0"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let header = |name: axum::http::HeaderName| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(header(axum::http::header::CONTENT_TYPE), "image/jpeg");
    assert_eq!(header(axum::http::header::VARY), "Cookie, Authorization");
    assert_eq!(
        header(axum::http::header::CACHE_CONTROL),
        "private, no-cache"
    );
    let etag = header(axum::http::header::ETAG);
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "strong entity-tag, got: {etag}"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"page-one");

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_page_answers_if_none_match_with_304_and_same_headers() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;
    let app = crate::backend::rest_router(AppState::new(pool));

    let first = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/pages/1"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(first.status(), StatusCode::OK);
    let etag = first
        .headers()
        .get(axum::http::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("200 carries an ETag")
        .to_string();

    let second = app
        .oneshot(get_with_bearer_and_if_none_match(
            &format!("/api/ebooks/{uuid}/pages/1"),
            &token,
            &etag,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        second
            .headers()
            .get(axum::http::header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some(etag.as_str()),
        "304 republishes the same validator"
    );
    assert_eq!(
        second
            .headers()
            .get(axum::http::header::VARY)
            .and_then(|v| v.to_str().ok()),
        Some("Cookie, Authorization"),
        "304 carries MEDIA_VARY so a shared cache can't cross users"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// The pages route is gated by `MediaAuthUser`, so a `?token=` query param
/// authenticates an otherwise-anonymous `<img>` fetch — the same contract
/// as `/file` and the covers routes.
#[tokio::test]
async fn api_get_ebook_page_returns_200_with_query_token() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_anon(&format!(
            "/api/ebooks/{uuid}/pages/0?token={token}"
        )))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_page_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/ebooks/does-not-exist/pages/0",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Two real pages exist (the AppleDouble entry is not one), so index 2 is
/// out of range — proving both the 404 contract and the hidden-entry
/// filter at the endpoint level.
#[tokio::test]
async fn api_get_ebook_page_returns_404_when_page_index_is_out_of_range() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/pages/2"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_page_returns_404_for_book_without_a_cbz_file() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _book_id, tmp) = seed_epub_on_disk(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/pages/0"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&tmp).ok();
}

/// Same induce-sqlx-error idiom as the `/file` 500 tests: drop `books` out
/// from under the live pool so `resolve_book_id_by_uuid` errors while the
/// auth gate keeps passing.
#[tokio::test]
async fn api_get_ebook_page_returns_500_when_resolve_book_id_by_uuid_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/any-uuid/pages/0", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Drives `get_ebook_page`'s second `internal(...)` arm — the sibling of
/// `api_get_ebook_file_returns_500_when_book_file_path_fails`: when
/// `db::resolve_book_id_by_uuid` returns `Ok(Some(_))` but
/// `db::book_file_path` returns `Err`, the handler must surface 500. Seed a
/// book row so the first call succeeds, then drop `book_files` so the
/// second call's JOIN errors out.
#[tokio::test]
async fn api_get_ebook_page_returns_500_when_book_file_path_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "66666666-6666-6666-6666-666666666666";
    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'B')")
        .bind(uuid)
        .bind(lib_id)
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap();

    // FKs off because `book_file_parts` references `book_files`; we want
    // the single DROP to succeed without cascading further. PRAGMA + DROP
    // pinned to a single connection — see the `/file` sibling test above
    // for the rationale.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE book_files")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/pages/0"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// A `book_files` row that points at bytes which aren't a zip archive is a
/// server-side failure, not a client error.
#[tokio::test]
async fn api_get_ebook_page_returns_500_when_archive_is_unreadable() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;
    std::fs::write(tmp.join("alpha.cbz"), b"not a zip").unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/pages/0"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

    std::fs::remove_dir_all(&tmp).ok();
}
