//! Hand-written `/api/*` REST routes for the mobile client.
//!
//! Web uses Dioxus server functions (see `omnibus_frontend::rpc`), mounted
//! automatically by `dioxus::server::router(App)`. These REST routes are
//! merged alongside them in `main.rs` so mobile's existing `reqwest` paths
//! keep working unchanged.

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use omnibus_frontend::{db, indexer, scanner};
use omnibus_shared::{Settings, ValueResponse};
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pool: SqlitePool,
}

impl AppState {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

pub fn rest_router(state: AppState) -> Router {
    Router::new()
        .route("/api/value", get(get_value))
        .route("/api/value/increment", post(increment_value))
        .route("/api/settings", get(get_settings))
        .route("/api/settings", post(post_settings))
        .route("/api/library", get(get_library))
        .route("/api/ebooks", get(get_ebooks))
        .route("/api/covers/{id}", get(get_cover))
        .with_state(state)
}

async fn get_value(State(state): State<AppState>) -> Response {
    match db::get_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read value: {error}"),
        )
            .into_response(),
    }
}

async fn increment_value(State(state): State<AppState>) -> Response {
    match db::increment_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to increment value: {error}"),
        )
            .into_response(),
    }
}

async fn get_settings(State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read settings: {error}"),
        )
            .into_response(),
    }
}

async fn post_settings(State(state): State<AppState>, Json(settings): Json<Settings>) -> Response {
    match db::set_settings(&state.pool, &settings).await {
        Ok(()) => match db::get_settings(&state.pool).await {
            Ok(updated) => {
                // Library path may have changed (and even when it hasn't,
                // the user has signalled they want to pick up on-disk
                // changes). Kick off a reindex in the background.
                if let Some(path) = updated.ebook_library_path.clone() {
                    let pool = state.pool.clone();
                    tokio::spawn(async move {
                        if let Err(e) = indexer::reindex(&pool, path).await {
                            eprintln!("post_settings: reindex failed: {e}");
                        }
                    });
                }
                Json(updated).into_response()
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

async fn get_ebooks(State(state): State<AppState>) -> Response {
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

async fn get_cover(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Covers are static per-book (new id on reindex), so cache
                // aggressively at the client.
                (header::CACHE_CONTROL, "public, max-age=86400"),
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

async fn get_library(State(state): State<AppState>) -> Response {
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
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn api_reads_and_increments_value() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/value")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: ValueResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.value, 0);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/value/increment")
                    .method("POST")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: ValueResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.value, 1);
    }

    #[tokio::test]
    async fn api_get_settings_returns_null_defaults() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let settings: Settings = serde_json::from_slice(&body).unwrap();
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn api_post_settings_persists_and_returns_saved_values() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));

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
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

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
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));

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
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("POST should succeed");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("GET should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let settings: Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(settings.ebook_library_path, Some("/my/ebooks".to_string()));
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn api_get_library_returns_empty_sections_when_paths_not_configured() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/library")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.path.is_none());
        assert_eq!(contents.ebooks.total_files, 0);
        assert!(contents.audiobooks.path.is_none());
        assert_eq!(contents.audiobooks.total_files, 0);
    }

    #[tokio::test]
    async fn api_get_library_reports_error_for_nonexistent_path() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/does/not/exist/omnibus_test".to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("set should succeed");
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/library")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.error.is_some());
        assert!(contents.audiobooks.path.is_none());
    }

    #[tokio::test]
    async fn api_get_ebooks_returns_empty_when_path_not_configured() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ebooks")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

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
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ebooks")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(lib.path.as_deref(), Some(path));
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    #[tokio::test]
    async fn api_get_covers_returns_not_found_for_missing_id() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = rest_router(AppState::new(pool));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/covers/9999")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
