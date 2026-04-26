//! Hand-written `/api/*` REST routes for the mobile client.
//!
//! Web uses Dioxus server functions (see `omnibus_frontend::rpc`), mounted
//! automatically by `dioxus::server::router(App)`. These REST routes are
//! merged alongside them in `main.rs` so mobile's existing `reqwest` paths
//! keep working unchanged.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use omnibus_db::{
    self as db, scanner,
    worker::{Task, Worker, WorkerConfig},
};
use omnibus_shared::{Settings, ValueResponse};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::{AdminUser, AuthUser};

#[derive(Clone)]
pub struct AppState {
    pool: SqlitePool,
    worker: Arc<Worker>,
}

impl AppState {
    pub fn new(pool: SqlitePool) -> Self {
        let worker = Worker::new(pool.clone(), WorkerConfig::default());
        Self { pool, worker }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn worker(&self) -> &Arc<Worker> {
        &self.worker
    }
}

pub fn rest_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/api/value", get(get_value))
        .route("/api/value/increment", post(increment_value))
        .route("/api/settings", get(get_settings))
        .route("/api/settings", post(post_settings))
        .route("/api/library", get(get_library))
        .route("/api/ebooks", get(get_ebooks))
        .route("/api/search", get(get_search))
        .route("/api/covers/{id}", get(get_cover))
        .with_state(state)
        // `AuthUser`/`AdminUser` read the pool from `Extension<SqlitePool>`.
        // Layer it here so the router is self-contained for integration
        // tests; in the live server `main.rs` adds the same Extension at
        // the top, which is harmless overlap.
        .layer(Extension(pool))
}

async fn get_value(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read value: {error}"),
        )
            .into_response(),
    }
}

async fn increment_value(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::increment_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to increment value: {error}"),
        )
            .into_response(),
    }
}

async fn get_settings(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read settings: {error}"),
        )
            .into_response(),
    }
}

async fn post_settings(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(settings): Json<Settings>,
) -> Response {
    match db::set_settings(&state.pool, &settings).await {
        Ok(()) => match db::get_settings(&state.pool).await {
            Ok(updated) => {
                // Library path may have changed (and even when it hasn't,
                // the user has signalled they want to pick up on-disk
                // changes). Hand the reindex to the shared Worker so the
                // per-path mutex serializes overlapping saves and the
                // scan_concurrency cap stays honored.
                let task_id = updated
                    .ebook_library_path
                    .clone()
                    .map(|library_path| state.worker.post(Task::Scan { library_path }));

                let mut response = Json(updated).into_response();
                #[cfg(debug_assertions)]
                if let Some(id) = task_id {
                    if let Ok(value) = id.to_string().parse::<axum::http::HeaderValue>() {
                        response
                            .headers_mut()
                            .insert("X-Omnibus-Worker-Task-Id", value);
                    }
                }
                #[cfg(not(debug_assertions))]
                let _ = task_id;
                response
            }
            Err(error) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read updated settings: {error}"),
            )
                .into_response(),
        },
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save settings: {error}"),
        )
            .into_response(),
    }
}

async fn get_ebooks(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read settings: {error}"),
            )
                .into_response();
        }
    };
    match db::library_from_db(&state.pool, settings.ebook_library_path.as_deref()).await {
        Ok(library) => Json(library).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read books: {error}"),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn get_search(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read settings: {error}"),
            )
                .into_response();
        }
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(omnibus_shared::EbookLibrary::default()).into_response();
    };
    match db::search_books(&state.pool, &path, &params.q).await {
        Ok(books) => Json(omnibus_shared::EbookLibrary {
            path: Some(path),
            books,
            error: None,
        })
        .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to search books: {error}"),
        )
            .into_response(),
    }
}

async fn get_cover(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Covers are static per-book (new id on reindex). Cached on
                // the client only — `private` + `Vary: Cookie` keep a shared
                // proxy from serving one user's covers to an unauthenticated
                // request on the same URL now that the endpoint is gated.
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read cover: {error}"),
        )
            .into_response(),
    }
}

async fn get_library(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => {
            let contents = scanner::scan_libraries(
                settings.ebook_library_path.as_deref(),
                settings.audiobook_library_path.as_deref(),
            );
            Json(contents).into_response()
        }
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read settings: {error}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support;

    /// Build a router + AppState wired against a fresh in-memory DB.
    async fn fixture() -> (Router, AppState, sqlx::SqlitePool) {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let state = AppState::new(pool.clone());
        let app = rest_router(state.clone());
        (app, state, pool)
    }

    /// Convenience: GET request with a bearer auth header.
    fn get_with_bearer(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    /// Convenience: anonymous GET (no auth header).
    fn get_anon(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    // -------------------------------------------------------------------
    // Happy paths — every protected route bootstraps the appropriate user.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_reads_and_increments_value() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .clone()
            .oneshot(get_with_bearer("/api/value", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: ValueResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.value, 0);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/value/increment")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: ValueResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.value, 1);
    }

    #[tokio::test]
    async fn api_get_settings_returns_null_defaults() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

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
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

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
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

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
    async fn api_get_library_returns_empty_sections_when_paths_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/library", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.path.is_none());
        assert_eq!(contents.ebooks.total_files, 0);
        assert!(contents.audiobooks.path.is_none());
        assert_eq!(contents.audiobooks.total_files, 0);
    }

    #[tokio::test]
    async fn api_get_library_reports_error_for_nonexistent_path() {
        let (_, _, pool) = fixture().await;
        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/does/not/exist/omnibus_test".to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("set should succeed");
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(get_with_bearer("/api/library", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.error.is_some());
        assert!(contents.audiobooks.path.is_none());
    }

    #[tokio::test]
    async fn api_get_ebooks_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    #[tokio::test]
    async fn api_get_ebooks_returns_empty_library_for_configured_path_without_index() {
        // /api/ebooks now reads from the books table; an unindexed path
        // surfaces as an empty library at that path, not an error.
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let path = "/does/not/exist/omnibus_ebook_test";
        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some(path.to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("set should succeed");
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(lib.path.as_deref(), Some(path));
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    #[tokio::test]
    async fn api_search_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
    }

    #[tokio::test]
    async fn api_search_rejects_missing_q_param() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search", &token))
            .await
            .expect("request should succeed");
        // axum's Query extractor returns 400 for missing required fields.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_get_covers_returns_not_found_for_missing_id() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/covers/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn post_settings_triggers_scan_via_worker() {
        use db::worker::TaskOutcome;

        let (app, state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

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

        match state.worker().await_completion(task_id).await {
            TaskOutcome::Ok => {}
            TaskOutcome::Err(e) => panic!("worker scan failed: {e}"),
        }

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

    // -------------------------------------------------------------------
    // 401 — anonymous request rejected by the per-route extractor (the
    // top-level `require_auth` middleware is not in this test stack;
    // these assertions confirm the extractor itself enforces the gate).
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_value_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/value")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_value_increment_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/value/increment")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
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
    async fn api_library_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/library")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_ebooks_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/ebooks")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_search_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/search?q=hello")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_covers_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/covers/1")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // 403 — non-admin authenticated user hits an admin-only route.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_get_settings_returns_403_when_not_admin() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "reader").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/settings", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_post_settings_returns_403_when_not_admin() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "reader").await;
        let token = test_support::bearer_token(&pool, user.id).await;
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
}
