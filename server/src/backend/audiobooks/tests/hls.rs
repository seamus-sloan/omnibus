//! The legacy HLS fallback — `playlist.m3u8`, `status` and `segments/{seg}`:
//! auth gating, the unknown uuid, the m3u8 body, the preparing / failed
//! status markers, segment-name validation, and the DB-failure paths
//! induced by dropping the first table each handler touches.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::*;
use super::seed_one_audiobook;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_audiobook_playlist_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/some-uuid/playlist.m3u8"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_audiobook_playlist_returns_404_for_unknown_uuid() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/does-not-exist/playlist.m3u8",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_audiobook_playlist_returns_m3u8_for_known_uuid() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_one_audiobook(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/playlist.m3u8"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(ct, "application/vnd.apple.mpegurl");
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = std::str::from_utf8(&body).unwrap();
    assert!(
        body_str.contains("#EXTM3U"),
        "manifest should contain #EXTM3U"
    );
    assert!(
        body_str.contains("seg-0000.ts"),
        "manifest should reference seg-0000.ts"
    );
}

#[tokio::test]
async fn api_get_audiobook_status_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/some-uuid/status"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_audiobook_status_returns_404_for_unknown_uuid() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/does-not-exist/status",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_audiobook_status_returns_preparing_when_not_transcoded() {
    // `hls::has_failed` reads `OMNIBUS_DATA_DIR` on every call, so this
    // must serialize against the `failed_marker` sibling test below —
    // `DataDirGuard` holds the shared env lock for its RAII lifetime
    // (see its doc comment in `backend::test_support` for why that's
    // safe to hold across `.await` in this test suite).
    let _data_dir = DataDirGuard::new("audiobook_status_preparing");

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_one_audiobook(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/status"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ready"], serde_json::Value::Bool(false));
    // New `state` field (#339 / Bug 4 of #338) — lets the UI
    // distinguish "preparing" from "failed".
    assert_eq!(json["state"], "preparing");
}

#[tokio::test]
async fn api_get_audiobook_status_returns_failed_when_failed_marker_present() {
    // Direct fs poke: write the `.failed` marker that
    // `cleanup_segment_dir` writes on a terminal ffmpeg failure, then
    // assert the status endpoint surfaces `state: "failed"` instead of
    // the legacy `ready:false, progress:0` shape that the UI couldn't
    // distinguish from "preparing". See sibling test above for why the
    // `OMNIBUS_DATA_DIR` swap must serialize against it.
    let data_dir = DataDirGuard::new("audiobook_status_failed");

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_one_audiobook(&pool).await;
    let book_id = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let book_dir = data_dir.path.join("hls").join(book_id.to_string());
    std::fs::create_dir_all(&book_dir).unwrap();
    std::fs::write(book_dir.join("audio64.failed"), "").unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/status"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["state"], "failed");
}

#[tokio::test]
async fn api_get_audiobook_segment_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/some-uuid/segments/seg-0000.ts"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn is_valid_segment_name_rejects_traversal_paths() {
    assert!(!is_valid_segment_name("../secret.txt"));
    assert!(!is_valid_segment_name("seg-000.ts"));
    assert!(!is_valid_segment_name("seg-00000.ts"));
    assert!(!is_valid_segment_name("seg-abcd.ts"));
    assert!(is_valid_segment_name("seg-0000.ts"));
    assert!(is_valid_segment_name("seg-9999.ts"));
}

#[tokio::test]
async fn is_valid_segment_name_accepts_uppercase_prefix_and_extension() {
    // Case-insensitive filesystems (APFS, NTFS) may surface the same file
    // as `.TS` or mixed-case; the validator must accept those forms.
    assert!(is_valid_segment_name("seg-0000.TS"));
    assert!(is_valid_segment_name("seg-1234.Ts"));
    assert!(is_valid_segment_name("SEG-0001.ts"));
}

// 5xx / DB-failure paths — induce sqlx errors by dropping the table
// that the first DB call in each handler touches. Auth gate uses
// `users`/`sessions` only, so it keeps passing; the handler's first
// query hits "no such table" and falls into `internal(...)` → 500.
// PRAGMA + DROP are pinned to a single pool connection because
// `PRAGMA foreign_keys` is per-connection in SQLite — executing via
// `&pool` would let the PRAGMA and the DROP land on different
// connections, leaving FK enforcement ON and causing the DROP to
// fail on FK constraints.
/// `get_audiobook_playlist` returns 500 when `resolve_audiobook` fails.
/// Drop the `books` table after seeding auth — the gate keeps passing
/// but the handler's JOIN hits "no such table: books".
#[tokio::test]
async fn api_get_audiobook_playlist_returns_500_when_db_fails() {
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
        .oneshot(get_with_bearer(
            "/api/audiobooks/any-uuid/playlist.m3u8",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// `get_audiobook_segment` returns 500 when `resolve_audiobook` fails.
/// Passes a valid segment name (`seg-0000.ts`) so the name-validation
/// guard passes before the DB call.
#[tokio::test]
async fn api_get_audiobook_segment_returns_500_when_db_fails() {
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
        .oneshot(get_with_bearer(
            "/api/audiobooks/any-uuid/segments/seg-0000.ts",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// `get_audiobook_status` returns 500 when `resolve_audiobook` fails.
#[tokio::test]
async fn api_get_audiobook_status_returns_500_when_db_fails() {
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
        .oneshot(get_with_bearer("/api/audiobooks/any-uuid/status", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
