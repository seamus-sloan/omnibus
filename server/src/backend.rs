use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use tower_http::trace::TraceLayer;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{db, frontend::Route};

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
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn index_page(State(state): State<AppState>) -> Response {
    render_page(&state.pool, Route::Landing {}).await
}

async fn settings_page(State(state): State<AppState>) -> Response {
    render_page(&state.pool, Route::Settings {}).await
}

async fn render_page(pool: &SqlitePool, route: Route) -> Response {
    match db::get_value(pool).await {
        Ok(value) => Html(crate::frontend::render_document(route, value)).into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load value: {error}"),
        )
            .into_response(),
    }
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
}
