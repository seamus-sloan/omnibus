//! Tests for settings read/write handlers.
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_settings_returns_null_defaults() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/settings", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&body).unwrap();
    assert_eq!(settings.ebook_library_path, None);
    assert_eq!(settings.audiobook_library_path, None);
}

#[tokio::test]
async fn api_post_settings_persists_and_returns_saved_values() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "ebook_library_path": "/books/ebooks",
        "audiobook_library_path": "/books/audio"
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        settings.ebook_library_path,
        Some("/books/ebooks".to_string())
    );
    assert_eq!(
        settings.audiobook_library_path,
        Some("/books/audio".to_string())
    );
}

#[tokio::test]
async fn api_get_settings_after_post_reflects_saved_values() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "ebook_library_path": "/my/ebooks",
        "audiobook_library_path": null
    });
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("POST should succeed");

    let response = app
        .oneshot(get_with_bearer("/api/settings", &token))
        .await
        .expect("GET should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(settings.ebook_library_path, Some("/my/ebooks".to_string()));
    assert_eq!(settings.audiobook_library_path, None);
}

#[tokio::test]
async fn post_settings_triggers_scan_via_worker() {
    let (app, state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Copy the playwright fixtures into an RAII temp dir before pointing
    // the indexer at them. Reindex now opts into cover-sidecar
    // materialization (F0.6) and would otherwise write `<stem>.{jpg|png}`
    // into the shared fixtures dir on every CI run. `tempfile::TempDir`
    // cleans itself up on Drop, so a panic before the assert below doesn't
    // leak under /tmp.
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/epubs/generated")
        .canonicalize()
        .expect("fixtures dir should resolve");
    assert!(source.is_dir(), "fixtures dir missing: {source:?}");
    let scratch = tempfile::tempdir().expect("create scratch dir");
    for entry in std::fs::read_dir(&source).expect("read fixtures dir") {
        let entry = entry.expect("fixture entry");
        if entry.file_type().expect("file type").is_file() {
            let dest = scratch.path().join(entry.file_name());
            std::fs::copy(entry.path(), dest).expect("copy fixture");
        }
    }
    let path_str = scratch.path().to_string_lossy().to_string();

    let body = serde_json::json!({
        "ebook_library_path": path_str,
        "audiobook_library_path": null,
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("POST should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let task_id: db::worker::TaskId = response
        .headers()
        .get("X-Omnibus-Worker-Task-Id")
        .expect("worker task id header should be set in debug builds")
        .to_str()
        .expect("header value should be ASCII")
        .parse()
        .expect("header value should be a u64");

    let outcome = state.worker().await_completion(task_id).await;
    assert!(
        matches!(outcome, TaskOutcome::Ok),
        "worker scan should succeed on a valid fixture dir, got {outcome:?}"
    );

    let response = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .expect("GET /api/ebooks should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert!(
        !lib.books.is_empty(),
        "worker should have indexed at least one book from {path_str}"
    );
    // `scratch` (and any cover sidecars the indexer materialized into
    // it) cleans up on Drop here.
}

/// When the worker's scan fails (here, because the configured library path
/// doesn't exist on disk), the `/api/reindex` handler must surface the
/// failure as a 500 via the `internal()` helper rather than panicking the
/// spawned task or returning a misleading 200. This is the live request
/// path the original `panic!("worker scan failed: ...")` was masking.
#[tokio::test]
async fn reindex_returns_500_when_worker_fails() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Point settings at a path that definitely doesn't exist on disk so
    // the worker's `Task::Scan` returns `TaskOutcome::Err`.
    let bogus_path = std::env::temp_dir()
        .join(format!(
            "omnibus-nonexistent-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        ))
        .to_string_lossy()
        .to_string();
    let settings = omnibus_shared::Settings {
        ebook_library_path: Some(bogus_path),
        audiobook_library_path: None,
    };
    db::set_settings(&pool, &settings)
        .await
        .expect("set_settings should persist the bogus path");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/reindex")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // Body must stay generic — the underlying scan error message is
    // logged via `tracing::error!` but never leaked on the wire (see
    // `internal()`).
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap_or("");
    assert_eq!(body, "internal server error");
}

#[tokio::test]
async fn reindex_returns_409_when_no_library_path_configured() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/reindex")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn reindex_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/reindex")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn reindex_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/reindex")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reindex_returns_200_when_scan_succeeds() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Same fixture-copy pattern as `post_settings_triggers_scan_via_worker`
    // so the scan finds a real EPUB and `Task::Scan` returns `Ok`. We use
    // a tempdir to keep the reindex from materializing cover sidecars
    // back into the shared fixtures directory.
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/epubs/generated")
        .canonicalize()
        .expect("fixtures dir should resolve");
    let scratch = tempfile::tempdir().expect("create scratch dir");
    for entry in std::fs::read_dir(&source).expect("read fixtures dir") {
        let entry = entry.expect("fixture entry");
        if entry.file_type().expect("file type").is_file() {
            let dest = scratch.path().join(entry.file_name());
            std::fs::copy(entry.path(), dest).expect("copy fixture");
        }
    }
    let settings = omnibus_shared::Settings {
        ebook_library_path: Some(scratch.path().to_string_lossy().to_string()),
        audiobook_library_path: None,
    };
    db::set_settings(&pool, &settings)
        .await
        .expect("set_settings should persist the fixture path");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/reindex")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn scan_library_returns_200_when_a_path_is_configured() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Fire-and-forget: the handler returns 200 as soon as the tasks are
    // queued, so a not-yet-existent path still yields 200 (unlike the
    // synchronous `/api/reindex`, which awaits the scan and would 500).
    let settings = omnibus_shared::Settings {
        ebook_library_path: Some("/some/library".to_string()),
        audiobook_library_path: None,
    };
    db::set_settings(&pool, &settings)
        .await
        .expect("set_settings should persist the path");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/scan-library")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn scan_library_returns_409_when_no_library_path_configured() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/scan-library")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn scan_library_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/scan-library")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn scan_library_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/scan-library")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_fts_rebuild_returns_200_for_admin() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Seed a book (which populates `books_fts` via `replace_books`), then
    // blow the FTS index away to simulate drift left by a failed
    // post-commit refresh. The rebuild must repair it: a 200 on the wire
    // and a `books_fts` row count back in sync with `books`.
    seed_book(&pool, "/lib", "Rebuildable Title").await;
    sqlx::query("DELETE FROM books_fts")
        .execute(&pool)
        .await
        .expect("clear books_fts to simulate drift");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/fts/rebuild")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let books_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .expect("count books");
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(&pool)
        .await
        .expect("count books_fts");
    assert_eq!(
        fts_count, books_count,
        "rebuild should re-derive one FTS row per book"
    );
}

#[tokio::test]
async fn post_fts_rebuild_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/fts/rebuild")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_fts_rebuild_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/fts/rebuild")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_get_settings_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/settings")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_settings_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let body = serde_json::json!({
        "ebook_library_path": null,
        "audiobook_library_path": null,
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_settings_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/settings", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_settings_returns_422_when_ebook_path_exceeds_max_len() {
    // Validates the `settings.validate()` guard: an over-long path must
    // return 422 with the typed error message, and the on-disk settings
    // must remain untouched.
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "a".repeat(omnibus_shared::PATH_MAX_LEN + 1);
    let body = serde_json::json!({
        "ebook_library_path": over_limit,
        "audiobook_library_path": null,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let msg = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        msg.contains("ebook_library_path"),
        "error body should name the offending field: {msg}"
    );
    assert!(
        msg.contains(&omnibus_shared::PATH_MAX_LEN.to_string()),
        "error body should name the limit: {msg}"
    );

    // 422 must short-circuit before `db::set_settings` runs.
    let stored = db::get_settings(&pool).await.expect("read settings");
    assert_eq!(stored.ebook_library_path, None);
    assert_eq!(stored.audiobook_library_path, None);
}

#[tokio::test]
async fn api_post_settings_returns_422_when_audiobook_path_exceeds_max_len() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "b".repeat(omnibus_shared::PATH_MAX_LEN + 1);
    let body = serde_json::json!({
        "ebook_library_path": null,
        "audiobook_library_path": over_limit,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let msg = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        msg.contains("audiobook_library_path"),
        "error body should name the offending field: {msg}"
    );
}

#[tokio::test]
async fn api_post_settings_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({
        "ebook_library_path": "/evil/path",
        "audiobook_library_path": null,
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
