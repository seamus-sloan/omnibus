use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use tower_http::trace::TraceLayer;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{db, frontend::Route, scanner};

#[derive(Clone)]
pub struct AppState {
    pool: SqlitePool,
}

impl AppState {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueResponse {
    pub value: i64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/settings", get(settings_page))
        .route("/api/value", get(get_value))
        .route("/api/value/increment", post(increment_value))
        .route("/api/settings", get(get_settings))
        .route("/api/settings", post(post_settings))
        .route("/api/library", get(get_library))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn index_page(State(state): State<AppState>) -> Response {
    match db::get_value(&state.pool).await {
        Ok(value) => Html(crate::frontend::render_document(
            Route::Landing {},
            value,
            db::Settings::default(),
        ))
        .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load value: {error}"),
        )
            .into_response(),
    }
}

async fn settings_page(State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load settings: {error}"),
            )
                .into_response();
        }
    };
    Html(crate::frontend::render_document(
        Route::Settings {},
        0,
        settings,
    ))
    .into_response()
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

async fn post_settings(
    State(state): State<AppState>,
    Json(settings): Json<db::Settings>,
) -> Response {
    match db::set_settings(&state.pool, &settings).await {
        Ok(()) => match db::get_settings(&state.pool).await {
            Ok(updated) => Json(updated).into_response(),
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
        let app = router(AppState::new(pool));

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
        let app = router(AppState::new(pool));

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
        let settings: db::Settings = serde_json::from_slice(&body).unwrap();
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn api_post_settings_persists_and_returns_saved_values() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = router(AppState::new(pool));

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
        let settings: db::Settings = serde_json::from_slice(&bytes).unwrap();
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
        let app = router(AppState::new(pool));

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
        let settings: db::Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(settings.ebook_library_path, Some("/my/ebooks".to_string()));
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn api_get_library_returns_empty_sections_when_paths_not_configured() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let app = router(AppState::new(pool));

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
        let contents: scanner::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.path.is_none());
        assert!(contents.ebooks.files.is_empty());
        assert!(contents.audiobooks.path.is_none());
        assert!(contents.audiobooks.files.is_empty());
    }

    #[tokio::test]
    async fn api_get_library_reports_error_for_nonexistent_path() {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        db::set_settings(
            &pool,
            &db::Settings {
                ebook_library_path: Some("/does/not/exist/omnibus_test".to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("set should succeed");
        let app = router(AppState::new(pool));

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
        let contents: scanner::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.error.is_some());
        assert!(contents.audiobooks.path.is_none());
    }
}
