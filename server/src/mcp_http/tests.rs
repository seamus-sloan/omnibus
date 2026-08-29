//! Tests for the hosted `/mcp` endpoint: the settings toggle gate (404 when
//! off, live without a restart), bearer auth (401 anonymous / invalid /
//! revoked), and a real streamable-HTTP handshake — initialize then
//! tools/list — with a valid API token, proving the rmcp transport serves
//! the stdio binary's tool layer over HTTP.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use omnibus_db as db;
use tower::ServiceExt;

use super::mcp_router;
use crate::auth::test_support::create_user;
use crate::backend::AppState;

async fn app() -> (Router, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    // The loopback URL is only dialed when a tool executes; initialize and
    // tools/list are answered by the router itself, so a closed port is fine.
    let router = mcp_router(AppState::new(pool.clone()), "http://127.0.0.1:1".into());
    (router, pool)
}

fn init_request(bearer: Option<&str>) -> Request<Body> {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    // rmcp rejects a Host-less request outright; every real HTTP/1.1
    // request carries one, so the tests do too.
    let mut req = Request::builder()
        .uri("/mcp")
        .method("POST")
        .header(header::HOST, "omnibus.example.com")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(token) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    req.body(Body::from(body)).unwrap()
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn api_token(pool: &sqlx::SqlitePool) -> String {
    let user = create_user(pool, "reader").await;
    db::auth::create_api_token(pool, user.id, "mcp")
        .await
        .unwrap()
        .raw_token
}

#[tokio::test]
async fn mcp_is_404_while_the_toggle_is_off() {
    // AC2: the default is off — even a valid token sees no MCP surface.
    let (app, pool) = app().await;
    let token = api_token(&pool).await;
    let res = app.oneshot(init_request(Some(&token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mcp_toggle_takes_effect_without_restart() {
    // AC5: one router instance, no rebuild — flipping the setting flips the
    // answer, because the handler reads it per request.
    let (app, pool) = app().await;
    let token = api_token(&pool).await;

    db::set_mcp_enabled(&pool, true).await.unwrap();
    let res = app
        .clone()
        .oneshot(init_request(Some(&token)))
        .await
        .unwrap();
    let status = res.status();
    assert_eq!(status, StatusCode::OK, "body: {}", body_string(res).await);

    db::set_mcp_enabled(&pool, false).await.unwrap();
    let res = app.oneshot(init_request(Some(&token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mcp_rejects_missing_invalid_and_revoked_bearers() {
    let (app, pool) = app().await;
    db::set_mcp_enabled(&pool, true).await.unwrap();

    // No Authorization header.
    let res = app.clone().oneshot(init_request(None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(res.headers().contains_key(header::WWW_AUTHENTICATE));

    // A bearer that resolves nowhere.
    let res = app
        .clone()
        .oneshot(init_request(Some(
            "omni_not-a-real-token-aaaaaaaaaaaaaaaaaaaaaaa",
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // A revoked API token (AC3 at the transport boundary).
    let user = create_user(&pool, "reader").await;
    let minted = db::auth::create_api_token(&pool, user.id, "mcp")
        .await
        .unwrap();
    db::auth::revoke_api_token_for_user(&pool, user.id, minted.token.id)
        .await
        .unwrap();
    let res = app
        .oneshot(init_request(Some(&minted.raw_token)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_serves_initialize_and_tools_list_with_an_api_token() {
    // AC1's transport half: a full streamable-HTTP handshake against the
    // mounted route. (Tool *execution* loops back over REST and is covered
    // by the stdio transport's own tool tests — the layer is shared.)
    let (app, pool) = app().await;
    db::set_mcp_enabled(&pool, true).await.unwrap();
    let token = api_token(&pool).await;

    let res = app
        .clone()
        .oneshot(init_request(Some(&token)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let session_id = res
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("initialize should mint a session id")
        .to_string();
    let body = body_string(res).await;
    assert!(
        body.contains("omnibus-mcp"),
        "initialize response should name the server: {body}"
    );

    // The initialized notification completes the handshake (202).
    let notified = Request::builder()
        .uri("/mcp")
        .method("POST")
        .header(header::HOST, "omnibus.example.com")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("mcp-session-id", &session_id)
        .body(Body::from(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ))
        .unwrap();
    let res = app.clone().oneshot(notified).await.unwrap();
    assert!(
        res.status().is_success(),
        "initialized notification: {}",
        res.status()
    );

    let list = Request::builder()
        .uri("/mcp")
        .method("POST")
        .header(header::HOST, "omnibus.example.com")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("mcp-session-id", &session_id)
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();
    let res = app.oneshot(list).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(
        body.contains("list_books"),
        "tools/list should carry the shared tool layer: {body}"
    );
}
