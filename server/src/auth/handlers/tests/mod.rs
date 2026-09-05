//! Tests for the auth register/login/logout/me handlers, split by
//! sub-topic into the sibling modules below; the router and JSON request
//! fixtures they share live here.

mod register_login;
mod sessions;
mod status_me;

use axum::{body::Body, http::Request};
use omnibus_db as db;

use super::*;

async fn app() -> (Router, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let router = auth_router(AppState::new(pool.clone()));
    (router, pool)
}

fn json_req(uri: &str, method: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}
