//! `GET /api/audiobooks/{uuid}/parts/{ordinal}`: auth gating (header or
//! `?token=`), the HLS-classified and out-of-range 404s, Range serving
//! with the right MIME type, streaming allowed without download
//! permission, and the DB-failure paths.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::*;
use super::seed_one_audiobook;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_audiobook_part_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/some-uuid/parts/0"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_audiobook_part_returns_404_when_book_is_hls_classified() {
    // Mixed-codec folders classify as HLS — the parts endpoint
    // must mirror that and 404 so clients can't bypass the
    // transcode pipeline by hitting `/parts` directly.
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        "/audiobooks",
        "MP3",
        &[
            (0, "Author/Book/01.mp3", 100.0),
            (1, "Author/Book/02.flac", 200.0),
        ],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/parts/0"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_audiobook_part_returns_404_for_out_of_range_ordinal() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        "/audiobooks",
        "MP3",
        &[(0, "Author/Book/01.mp3", 100.0)],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/parts/9"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_audiobook_part_serves_range_request_with_correct_mime() {
    // ServeFile reads the real file from disk, so we write a small
    // payload in a temp dir and point the seeded library at it. The
    // test asserts both Range support (206 + Content-Range) and the
    // mime override (`audio/mpeg`, not `application/octet-stream`).
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let file_path = dir.path().join("Author/Book/01.mp3");
    // 100-byte payload of alternating bytes so we can verify the
    // sliced Range bytes are exactly the slice we asked for.
    let payload: Vec<u8> = (0u8..100).collect();
    std::fs::File::create(&file_path)
        .unwrap()
        .write_all(&payload)
        .unwrap();

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        &library_path,
        "MP3",
        &[(0, "Author/Book/01.mp3", 60.0)],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let req = axum::http::Request::builder()
        .uri(format!("/api/audiobooks/{uuid}/parts/0"))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(axum::http::header::RANGE, "bytes=10-19")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    let ct = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(ct, "audio/mpeg");
    let cr = res
        .headers()
        .get(axum::http::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(cr, "bytes 10-19/100");
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), &payload[10..=19]);
}

/// AC1 (#1810): `/parts/{ordinal}` is the in-app playback stream, not a
/// download — it must stay reachable even for a `can_download = 0` user.
/// Only the `Content-Disposition: attachment` `/download` route enforces
/// the flag.
#[tokio::test]
async fn api_get_audiobook_part_returns_200_when_user_cannot_download() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let file_path = dir.path().join("Author/Book/01.mp3");
    std::fs::File::create(&file_path)
        .unwrap()
        .write_all(&[0u8; 10])
        .unwrap();

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    revoke_can_download(&pool, user.id).await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        &library_path,
        "MP3",
        &[(0, "Author/Book/01.mp3", 60.0)],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/parts/0"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_get_audiobook_part_serves_with_query_token_no_header() {
    // Mobile plays audio through a WebView `<audio src>`, whose fetch carries
    // neither the native bearer header nor a session cookie — only `?token=`.
    // The part handler must authenticate on that query param alone (via
    // `MediaAuthUser`), matching how covers/thumbs already behave.
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let file_path = dir.path().join("Author/Book/01.mp3");
    let payload: Vec<u8> = (0u8..100).collect();
    std::fs::File::create(&file_path)
        .unwrap()
        .write_all(&payload)
        .unwrap();

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        &library_path,
        "MP3",
        &[(0, "Author/Book/01.mp3", 60.0)],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let req = axum::http::Request::builder()
        .uri(format!("/api/audiobooks/{uuid}/parts/0?token={token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(ct, "audio/mpeg");
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), payload.as_slice());
}

/// `get_audiobook_part` returns 500 when `resolve_audiobook` fails.
#[tokio::test]
async fn api_get_audiobook_part_returns_500_when_db_fails() {
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
        .oneshot(get_with_bearer("/api/audiobooks/any-uuid/parts/0", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// `get_audiobook_part` returns 500 when `get_parts` fails. Seed an mp3
/// audiobook so `resolve_audiobook` succeeds, then drop
/// `book_file_parts` so the subsequent `get_parts` call errors out.
#[tokio::test]
async fn api_get_audiobook_part_returns_500_when_get_parts_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_one_audiobook(&pool).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE book_file_parts")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/parts/0"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
