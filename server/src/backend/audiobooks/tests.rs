//! Integration tests for the audiobook streaming endpoints. Covers the
//! direct-play manifest, Range-served `parts/{ordinal}` source files, and
//! the legacy HLS fallback (`playlist.m3u8`, `segments/{seg}`, `status`),
//! including auth gating, 4xx client errors, and 5xx DB-failure paths.

use std::sync::Mutex;

use axum::http::StatusCode;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Serializes the status-endpoint tests that mutate `OMNIBUS_DATA_DIR`.
/// `hls::has_failed` reads the env var on every call, so two tests
/// pointing at different tempdirs will race and one will see the
/// other's `.failed` marker. Mirrors the `ENV_LOCK` pattern in
/// `db/src/thumbs.rs`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Seed one audiobook book + book_files + book_file_parts row for tests.
async fn seed_one_audiobook(pool: &sqlx::SqlitePool) -> String {
    let lib_id = sqlx::query("INSERT INTO libraries (path, display_name) VALUES (?, 'audiobooks')")
        .bind("/audiobooks")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'PK')")
            .bind(uuid)
            .bind(lib_id)
            .bind("/audiobooks")
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime) \
         VALUES (?, 'MP3', 'the-princess-knight', 100, '')",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, 'ch01.mp3', 50, 0, 300.0)",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
    uuid.to_string()
}

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
// Std mutex held across awaits is the intent — env vars are
// process-global and we serialize sibling tests that mutate
// `OMNIBUS_DATA_DIR`. Safe under tokio's current-thread test runtime.
#[allow(clippy::await_holding_lock)]
async fn api_get_audiobook_status_returns_preparing_when_not_transcoded() {
    // Hold the env lock for the whole test so the `failed_marker`
    // sibling test below can't interleave its `.failed` write into
    // our `OMNIBUS_DATA_DIR`. Tempdir keeps us isolated from any
    // pre-existing `./data/hls/*/audio64.failed` files on the host.
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var("OMNIBUS_DATA_DIR").ok();
    // SAFETY: held under ENV_LOCK; no other thread mutates the env.
    unsafe {
        std::env::set_var("OMNIBUS_DATA_DIR", dir.path());
    }

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

    unsafe {
        match prev {
            Some(v) => std::env::set_var("OMNIBUS_DATA_DIR", v),
            None => std::env::remove_var("OMNIBUS_DATA_DIR"),
        }
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // see sibling test for rationale
async fn api_get_audiobook_status_returns_failed_when_failed_marker_present() {
    // Direct fs poke: write the `.failed` marker that
    // `cleanup_segment_dir` writes on a terminal ffmpeg failure, then
    // assert the status endpoint surfaces `state: "failed"` instead of
    // the legacy `ready:false, progress:0` shape that the UI couldn't
    // distinguish from "preparing".
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var("OMNIBUS_DATA_DIR").ok();
    // SAFETY: held under ENV_LOCK; no other thread mutates the env.
    unsafe {
        std::env::set_var("OMNIBUS_DATA_DIR", dir.path());
    }

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_one_audiobook(&pool).await;
    let book_id = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let book_dir = dir.path().join("hls").join(book_id.to_string());
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

    unsafe {
        match prev {
            Some(v) => std::env::set_var("OMNIBUS_DATA_DIR", v),
            None => std::env::remove_var("OMNIBUS_DATA_DIR"),
        }
    }
}

/// Seed an audiobook with N custom parts. Used by the manifest tests
/// to exercise direct, hls, and per-ordinal-lookup code paths from a
/// single helper. `library_path` becomes the prefix that
/// `get_audiobook_part` joins to each part's filename.
async fn seed_audiobook_with_parts(
    pool: &sqlx::SqlitePool,
    library_path: &str,
    format: &str,
    parts: &[(i64, &str, f64)],
) -> String {
    let lib_id = sqlx::query("INSERT INTO libraries (path, display_name) VALUES (?, 'lib')")
        .bind(library_path)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = format!("uuid-{}-{}", format.to_lowercase(), parts.len());
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'T')")
            .bind(&uuid)
            .bind(lib_id)
            .bind(library_path)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime) \
         VALUES (?, ?, 'book', 0, '')",
    )
    .bind(book_id)
    .bind(format)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    for (ordinal, filename, duration) in parts {
        sqlx::query(
            "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
             VALUES (?, ?, ?, 0, 0, ?)",
        )
        .bind(file_id)
        .bind(*ordinal)
        .bind(*filename)
        .bind(*duration)
        .execute(pool)
        .await
        .unwrap();
    }
    uuid
}

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
async fn is_valid_segment_name_accepts_uppercase_extension() {
    // Case-insensitive filesystems (APFS, NTFS) may surface the same file
    // as `.TS` or mixed-case; the validator must accept those forms.
    assert!(is_valid_segment_name("seg-0000.TS"));
    assert!(is_valid_segment_name("seg-1234.Ts"));
    assert!(is_valid_segment_name("SEG-0001.ts"));
}

// -------------------------------------------------------------------
// 5xx / DB-failure paths — induce sqlx errors by dropping the table
// that the first DB call in each handler touches. Auth gate uses
// `users`/`sessions` only, so it keeps passing; the handler's first
// query hits "no such table" and falls into `internal(...)` → 500.
// PRAGMA + DROP are pinned to a single pool connection because
// `PRAGMA foreign_keys` is per-connection in SQLite — executing via
// `&pool` would let the PRAGMA and the DROP land on different
// connections, leaving FK enforcement ON and causing the DROP to
// fail on FK constraints.
// -------------------------------------------------------------------

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
