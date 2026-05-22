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
use omnibus_shared::{MetadataOverrides, Settings, ValueResponse};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::{AdminUser, AuthUser};

/// Generic 500 response that never leaks internal error details to the wire.
/// The full error is logged server-side via `tracing::error!` so it remains
/// available in structured logs; the client sees only the boilerplate body.
fn internal<E: std::fmt::Display>(context: &'static str, e: E) -> Response {
    tracing::error!(error = %e, context = context, "internal server error");
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "internal server error",
    )
        .into_response()
}

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
        .route("/api/_health", get(get_health))
        .route("/api/value", get(get_value))
        .route("/api/value/increment", post(increment_value))
        .route("/api/settings", get(get_settings))
        .route("/api/settings", post(post_settings))
        .route("/api/library", get(get_library))
        .route("/api/ebooks", get(get_ebooks))
        .route("/api/ebooks/{id}", get(get_ebook_by_id))
        .route("/api/ebooks/{id}/overrides", post(post_ebook_overrides))
        .route(
            "/api/ebooks/{id}/overrides/delete",
            post(delete_ebook_overrides),
        )
        .route("/api/ebooks/{id}/cover", post(post_ebook_cover))
        .route("/api/search", get(get_search))
        .route("/api/covers/{id}", get(get_cover))
        .route("/api/thumbs/{id}/{size}", get(get_thumb))
        .with_state(state)
        // `AuthUser`/`AdminUser` read the pool from `Extension<SqlitePool>`.
        // Layer it here so the router is self-contained for integration
        // tests; in the live server `main.rs` adds the same Extension at
        // the top, which is harmless overlap.
        .layer(Extension(pool))
}

/// Process-start build id. Captured once and preserved for the lifetime of
/// the process — so any HMR cycle that restarts the server (the only way
/// `dx serve` rebuilds Rust changes) produces a new id. Claude's
/// `ui-validate` skill polls this to know when a rebuild has actually
/// landed.
///
/// `main.rs` calls [`init_build_id`] eagerly during boot so the id is set
/// before any request can read it; this keeps the doc accurate ("process
/// start" rather than "first health check"). Calling `build_id()` later
/// returns the same value because `OnceLock::get_or_init` is idempotent.
pub fn build_id() -> u128 {
    *BUILD_ID.get_or_init(now_millis)
}

/// Eagerly initialize [`build_id`] so the returned timestamp truly
/// represents process-start rather than first-call. Idempotent.
pub fn init_build_id() {
    let _ = BUILD_ID.get_or_init(now_millis);
}

static BUILD_ID: std::sync::OnceLock<u128> = std::sync::OnceLock::new();

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Unauthenticated liveness + fingerprint endpoint. The `app` field lets
/// `scripts/dev-server-up.sh` distinguish an omnibus instance from some
/// other process that happens to bind the same port. Whitelisted in
/// `auth::gate::require_auth` so it remains reachable without a session.
async fn get_health() -> Response {
    Json(serde_json::json!({
        "app": "omnibus",
        "status": "ok",
        "build_id": build_id().to_string(),
    }))
    .into_response()
}

async fn get_value(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => internal("read value", error),
    }
}

async fn increment_value(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::increment_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => internal("increment value", error),
    }
}

async fn get_settings(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => internal("read settings", error),
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
            Err(error) => internal("read updated settings", error),
        },
        Err(error) => internal("save settings", error),
    }
}

async fn get_ebooks(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    match db::library_from_db(&state.pool, settings.ebook_library_path.as_deref()).await {
        Ok(library) => Json(library).into_response(),
        Err(error) => internal("read books", error),
    }
}

async fn get_ebook_by_id(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read book", error),
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
        Err(error) => return internal("read settings", error),
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
        Err(error) => internal("search books", error),
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
        Err(error) => internal("read cover", error),
    }
}

async fn get_thumb(
    _user: AuthUser,
    State(state): State<AppState>,
    Path((id, size_str)): Path<(i64, String)>,
) -> Response {
    let size: db::ThumbSize = match size_str.parse() {
        Ok(s) => s,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "invalid size; use sm, md, or lg",
            )
                .into_response();
        }
    };

    let last_modified_epoch = match db::get_last_modified_epoch(&state.pool, id).await {
        Ok(Some(ts)) => ts,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("read last_modified_epoch", e),
    };

    // Cache hit: thumb exists and is fresh. Use async I/O here so a hot
    // `srcset` grid doesn't pin tokio worker threads on the synchronous read.
    let thumb_path = db::thumb_path_for(id, size);
    if !db::thumbs::is_stale_async(id, size, last_modified_epoch).await {
        if let Ok(bytes) = tokio::fs::read(&thumb_path).await {
            return (
                [
                    (header::CONTENT_TYPE, "image/webp"),
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                    (header::VARY, "Cookie"),
                ],
                bytes,
            )
                .into_response();
        }
    }

    // Cache miss or stale: fetch the original cover first so we only queue
    // generation when there's actually something to thumbnail. Queuing for
    // a coverless book just produces a guaranteed `no cover for book …`
    // worker error on every request, polluting the log.
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => {
            state.worker.post(db::worker::Task::GenerateThumbs {
                book_id: id,
                last_modified_epoch,
            });
            (
                [
                    (header::CONTENT_TYPE, mime.as_str()),
                    // Short TTL: browser will re-fetch after ~5 s when the WebP is ready.
                    (header::CACHE_CONTROL, "private, max-age=5"),
                    (header::VARY, "Cookie"),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => axum::http::StatusCode::ACCEPTED.into_response(),
        Err(e) => internal("cover fetch for thumb", e),
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
        Err(error) => internal("read settings", error),
    }
}

// ---------------------------------------------------------------------------
// F5.1 Metadata overrides (REST — mobile client).
// ---------------------------------------------------------------------------

/// Save metadata overrides for a book. Requires `can_edit` or admin.
async fn post_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(overrides): Json<MetadataOverrides>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    let uuid = match db::get_book_uuid(&state.pool, id).await {
        Ok(Some(u)) => u,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("get_book_uuid", e),
    };
    // Merge incoming overrides with any existing ones so that a second edit
    // that only touches field B doesn't wipe a prior override on field A.
    let merged = match db::get_metadata_overrides(&state.pool, &uuid).await {
        Ok(Some((existing, _))) => existing.merge(&overrides),
        Ok(None) => overrides,
        Err(e) => return internal("get_metadata_overrides", e),
    };
    if let Err(e) = db::upsert_metadata_overrides(&state.pool, &uuid, &merged, false, user.id).await
    {
        return internal("upsert_metadata_overrides", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Delete metadata overrides for a book, reverting to scanned values.
async fn delete_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    let uuid = match db::get_book_uuid(&state.pool, id).await {
        Ok(Some(u)) => u,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("get_book_uuid", e),
    };
    if let Err(e) = db::delete_metadata_overrides(&state.pool, &uuid).await {
        return internal("delete_metadata_overrides", e);
    }
    db::delete_override_cover(&uuid);
    db::thumbs::invalidate_thumbs(id);
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Upload a replacement cover image for a book. Multipart form with a single
/// `cover` field containing the image bytes.
async fn post_ebook_cover(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }

    let uuid = match db::get_book_uuid(&state.pool, id).await {
        Ok(Some(u)) => u,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("get_book_uuid", e),
    };

    // Extract the cover field from the multipart body.
    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "cover" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "cover must be an image",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "cover must be under 10 MB",
                            )
                                .into_response();
                        }
                        break (content_type, b);
                    }
                    Err(e) => return internal("read cover field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'cover' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    // Write the override cover to disk.
    if let Err(e) = db::write_override_cover(&uuid, &mime, &bytes) {
        return internal("write_override_cover", e);
    }

    // Mark the overrides table with has_cover_override = 1. Preserve existing
    // field overrides if any.
    let existing_overrides = match db::get_metadata_overrides(&state.pool, &uuid).await {
        Ok(Some((ov, _))) => ov,
        Ok(None) => MetadataOverrides::default(),
        Err(e) => return internal("get_metadata_overrides", e),
    };
    if let Err(e) =
        db::upsert_metadata_overrides(&state.pool, &uuid, &existing_overrides, true, user.id).await
    {
        return internal("upsert_metadata_overrides", e);
    }

    // Invalidate thumb cache so next request regenerates from new cover.
    db::thumbs::invalidate_thumbs(id);

    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
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
    // /api/_health — unauthenticated liveness + fingerprint.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_health_returns_200_unauth_with_app_and_build_id() {
        let (app, _state, _pool) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/_health"))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON body");
        assert_eq!(body["app"], "omnibus");
        assert_eq!(body["status"], "ok");
        let build_id = body["build_id"]
            .as_str()
            .expect("build_id should be string");
        assert!(
            build_id.chars().all(|c| c.is_ascii_digit()),
            "build_id should be all digits, got {build_id:?}"
        );
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
    async fn api_get_ebook_returns_200_with_metadata() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("Alpha Book".into()),
                    ..Default::default()
                },
                cover: None,
            }],
        )
        .await
        .unwrap();

        let books = db::list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        let response = app
            .oneshot(get_with_bearer(&format!("/api/ebooks/{id}"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.title.as_deref(), Some("Alpha Book"));
        assert_eq!(book.id, id);
    }

    #[tokio::test]
    async fn api_get_ebook_returns_404_for_unknown_id() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/ebooks/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_ebook_returns_401_when_anonymous() {
        let (app, _state, _pool) = fixture().await;
        let response = app
            .oneshot(get_anon("/api/ebooks/1"))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
    // /api/thumbs — thumbnail pipeline endpoint
    // -------------------------------------------------------------------

    /// Seed a book row with `has_cover = 0`. Returns the inserted book id.
    async fn seed_book_no_cover(pool: &sqlx::SqlitePool) -> i64 {
        // Insert a minimal library row first (FK requirement).
        sqlx::query(
            "INSERT OR IGNORE INTO libraries(path, display_name) VALUES ('/test/library', 'Test')",
        )
        .execute(pool)
        .await
        .expect("insert library");
        let library_id: i64 =
            sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/test/library'")
                .fetch_one(pool)
                .await
                .expect("library id");
        // Use a fixed UUID; each test gets its own in-memory pool so there is
        // no collision risk.
        sqlx::query(
            "INSERT INTO books(uuid, library_id, path, title, has_cover) VALUES (?, ?, ?, ?, 0)",
        )
        .bind("00000000-0000-0000-0000-000000000001")
        .bind(library_id)
        .bind("/test/library/no-cover.epub")
        .bind("No Cover Book")
        .execute(pool)
        .await
        .expect("insert book")
        .last_insert_rowid()
    }

    #[tokio::test]
    async fn api_thumbs_returns_400_for_bad_size() {
        let (app, _, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/thumbs/1/xxl", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_thumbs_returns_404_for_missing_book() {
        let (app, _, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/thumbs/9999/md", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_thumbs_returns_202_for_book_without_cover() {
        let (_, _, pool) = fixture().await;
        let book_id = seed_book_no_cover(&pool).await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let app = rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/thumbs/{book_id}/md"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn api_thumbs_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/thumbs/1/md")).await.unwrap();
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

    // -------------------------------------------------------------------
    // 500 — handler error path returns a generic body, never leaks the
    // underlying sqlx error message. Regression test for #78.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_value_500_body_is_generic_and_never_leaks_db_details() {
        let (_, _, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        // Force `db::get_value` to fail by dropping the table it reads.
        // Auth setup above completed before the drop, so the request still
        // passes the `AuthUser` extractor and reaches `get_value`.
        sqlx::query("DROP TABLE app_state")
            .execute(&pool)
            .await
            .expect("drop app_state");

        let app = rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer("/api/value", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).expect("utf-8 body");
        assert_eq!(body, "internal server error");
        assert!(
            !body.contains("app_state") && !body.contains("sqlx") && !body.contains("SQL"),
            "500 body must not leak internal error details, got {body:?}"
        );
    }
}
