//! `GET /api/audiobooks/{uuid}/manifest`: auth gating, the unknown uuid,
//! direct play for a single M4B or an MP3 folder with the resolved file and
//! audio-file count, the HLS classification when any part is FLAC, and the
//! DB-failure paths.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::*;
use super::seed_one_audiobook;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_audiobook_manifest_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/some-uuid/manifest"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_audiobook_manifest_returns_404_for_unknown_uuid() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/does-not-exist/manifest",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_audiobook_manifest_returns_direct_for_single_m4b() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        "/audiobooks",
        "M4B",
        &[(0, "Author/Book.m4b", 3600.0)],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/manifest"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "direct");
    assert_eq!(json["total_duration_seconds"].as_f64(), Some(3600.0));
    assert_eq!(json["parts"].as_array().unwrap().len(), 1);
    assert_eq!(json["parts"][0]["ordinal"], 0);
    assert_eq!(
        json["parts"][0]["url"],
        format!("/api/audiobooks/{uuid}/parts/0"),
    );
    assert_eq!(json["parts"][0]["mime"], "audio/mp4");
}

#[tokio::test]
async fn api_get_audiobook_manifest_returns_direct_for_mp3_folder() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        "/audiobooks",
        "MP3",
        &[
            (0, "Author/Book/01.mp3", 1800.0),
            (1, "Author/Book/02.mp3", 1800.0),
            (2, "Author/Book/03.mp3", 1800.0),
        ],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/manifest"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "direct");
    assert_eq!(json["total_duration_seconds"].as_f64(), Some(5400.0));
    let parts = json["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[2]["url"], format!("/api/audiobooks/{uuid}/parts/2"));
    assert_eq!(parts[0]["mime"], "audio/mpeg");
}

#[tokio::test]
async fn api_get_audiobook_manifest_reports_resolved_file_and_audio_file_count() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        "/audiobooks",
        "M4B",
        &[(0, "Author/Book/Part1.m4b", 3600.0)],
    )
    .await;
    // Second audio file on the same book — the multi-file shape (#1888).
    let book_id = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let second_file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, ordinal) \
         VALUES (?, 'M4B', 'book-2', 0, 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, 'Author/Book/Part2.m4b', 0, 0, 1800.0)",
    )
    .bind(second_file_id)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/manifest?file_id={second_file_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["book_file_id"].as_i64(), Some(second_file_id));
    assert_eq!(json["audio_file_count"].as_i64(), Some(2));

    // Without `?file_id=` the manifest resolves — and names — the default
    // (lowest-ordinal) file.
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/manifest"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(json["book_file_id"].as_i64(), Some(second_file_id));
    assert!(json["book_file_id"].as_i64().unwrap() > 0);
    assert_eq!(json["audio_file_count"].as_i64(), Some(2));
}

#[tokio::test]
async fn api_get_audiobook_manifest_returns_hls_when_any_part_is_flac() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // One flac in an otherwise mp3 folder forces the whole book
    // through HLS — the cross-part timeline math doesn't have to
    // deal with mid-book codec switches that way.
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
            &format!("/api/audiobooks/{uuid}/manifest"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["mode"], "hls");
    assert_eq!(
        json["playlist_url"],
        format!("/api/audiobooks/{uuid}/playlist.m3u8"),
    );
}

/// `get_audiobook_manifest` returns 500 when `resolve_audiobook` fails.
#[tokio::test]
async fn api_get_audiobook_manifest_returns_500_when_db_fails() {
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
        .oneshot(get_with_bearer("/api/audiobooks/any-uuid/manifest", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// `get_audiobook_manifest` returns 500 when `get_parts` fails. Seed a
/// real audiobook so `resolve_audiobook` succeeds, then drop
/// `book_file_parts` so the subsequent `get_parts` call errors out.
#[tokio::test]
async fn api_get_audiobook_manifest_returns_500_when_get_parts_fails() {
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
            &format!("/api/audiobooks/{uuid}/manifest"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
