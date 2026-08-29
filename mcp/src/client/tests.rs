use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use super::*;
use crate::config::Config;

/// Stub instance state: counts logins, remembers which tokens are live, and
/// records the last login body + request headers / write body for assertions.
#[derive(Default)]
struct Stub {
    logins: AtomicUsize,
    valid: Mutex<HashSet<String>>,
    last_login_body: Mutex<Option<serde_json::Value>>,
    last_get_headers: Mutex<Option<(Option<String>, Option<String>)>>,
    last_write_body: Mutex<Option<serde_json::Value>>,
    deletes: AtomicUsize,
}

impl Stub {
    fn revoke_all(&self) {
        self.valid.lock().unwrap().clear();
    }
}

async fn login(
    State(stub): State<Arc<Stub>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let n = stub.logins.fetch_add(1, Ordering::SeqCst) + 1;
    let token = format!("token-{n}");
    stub.valid.lock().unwrap().insert(token.clone());
    *stub.last_login_body.lock().unwrap() = Some(body);
    Json(serde_json::json!({
        "user": {
            "id": 1, "username": "reader", "is_admin": false,
            "can_upload": false, "can_edit": false, "can_download": true
        },
        "token": token,
    }))
}

async fn protected(
    State(stub): State<Arc<Stub>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    *stub.last_get_headers.lock().unwrap() = Some((auth.clone(), ua));
    let token = auth
        .and_then(|a| a.strip_prefix("Bearer ").map(str::to_string))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !stub.valid.lock().unwrap().contains(&token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(serde_json::json!({ "value": 42 })))
}

#[derive(Debug, PartialEq, serde::Deserialize)]
struct Payload {
    value: i64,
}

/// Bearer check shared by the stub's write routes: `Ok(token)` when the
/// header carries a live token, 401 otherwise.
fn check_bearer(stub: &Stub, headers: &HeaderMap) -> Result<(), StatusCode> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !stub.valid.lock().unwrap().contains(token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

/// Allowlisted write route (`POST /api/scan/resolve`): records the body.
async fn write_thing(
    State(stub): State<Arc<Stub>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    check_bearer(&stub, &headers)?;
    *stub.last_write_body.lock().unwrap() = Some(body);
    Ok(Json(serde_json::json!({ "value": 42 })))
}

/// Allowlisted 204 route (`DELETE /api/physical/{uuid}/wishlist`).
async fn delete_thing(
    State(stub): State<Arc<Stub>>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    check_bearer(&stub, &headers)?;
    stub.deletes.fetch_add(1, Ordering::SeqCst);
    Ok(StatusCode::NO_CONTENT)
}

/// Boot a stub instance and return `(client, stub)`.
async fn stub_client() -> (OmnibusClient, Arc<Stub>) {
    let stub = Arc::new(Stub::default());
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/thing", get(protected))
        .route("/api/scan/resolve", post(write_thing))
        .route(
            "/api/scan/search",
            post(|| async { (StatusCode::BAD_REQUEST, "query is required") }),
        )
        .route("/api/physical/{uuid}/wishlist", delete(delete_thing))
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    (client, stub)
}

#[tokio::test]
async fn get_json_logs_in_lazily_and_sends_bearer_and_user_agent() {
    let (client, stub) = stub_client().await;

    let body: Payload = client.get_json("/api/thing", &[]).await.unwrap();
    assert_eq!(body, Payload { value: 42 });
    assert_eq!(stub.logins.load(Ordering::SeqCst), 1);

    // Login body identifies the MCP client: bearer session + dedicated device.
    let login_body = stub.last_login_body.lock().unwrap().clone().unwrap();
    assert_eq!(login_body["client_kind"], "bearer");
    assert_eq!(login_body["device_name"], DEVICE_NAME);
    assert_eq!(login_body["username"], "reader");

    // Requests carry the distinct User-Agent for log separability.
    let (auth, ua) = stub.last_get_headers.lock().unwrap().clone().unwrap();
    assert_eq!(auth.unwrap(), "Bearer token-1");
    assert_eq!(ua.unwrap(), USER_AGENT);
}

#[tokio::test]
async fn get_json_relogs_in_transparently_when_the_session_expired() {
    let (client, stub) = stub_client().await;

    let _: Payload = client.get_json("/api/thing", &[]).await.unwrap();
    // Simulate the 7-day idle expiry: the server no longer knows the token.
    stub.revoke_all();

    let body: Payload = client.get_json("/api/thing", &[]).await.unwrap();
    assert_eq!(body, Payload { value: 42 });
    assert_eq!(stub.logins.load(Ordering::SeqCst), 2);
    let (auth, _) = stub.last_get_headers.lock().unwrap().clone().unwrap();
    assert_eq!(auth.unwrap(), "Bearer token-2");
}

#[tokio::test]
async fn get_json_races_share_one_relogin_after_expiry() {
    let (client, stub) = stub_client().await;
    let client = Arc::new(client);

    let _: Payload = client.get_json("/api/thing", &[]).await.unwrap();
    // Simulate the idle expiry, then race ten tool calls in from the same
    // dead session: the login singleflight means one fresh login serves
    // them all instead of ten sessions piling up on the instance.
    stub.revoke_all();

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let c = client.clone();
            tokio::spawn(async move { c.get_json::<Payload>("/api/thing", &[]).await })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.await.unwrap().unwrap(), Payload { value: 42 });
    }
    assert_eq!(stub.logins.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn get_json_returns_login_failed_for_rejected_credentials() {
    let stub = Arc::new(Stub::default());
    let app = Router::new()
        .route(
            "/api/auth/login",
            post(|| async { (StatusCode::UNAUTHORIZED, "invalid credentials") }),
        )
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "reader".into(),
        password: "wrong".into(),
    })
    .unwrap();

    let err = client
        .get_json::<Payload>("/api/thing", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::LoginFailed { status: 401 }));
}

#[tokio::test]
async fn get_json_returns_no_token_when_the_server_issues_a_cookie_session() {
    let app = Router::new().route(
        "/api/auth/login",
        post(|| async {
            Json(serde_json::json!({
                "user": {
                    "id": 1, "username": "reader", "is_admin": false,
                    "can_upload": false, "can_edit": false, "can_download": true
                },
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();

    let err = client
        .get_json::<Payload>("/api/thing", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::NoToken));
}

#[tokio::test]
async fn get_json_returns_status_without_relogin_for_a_non_401_failure() {
    let (client, stub) = stub_client().await;
    let err = client
        .get_json::<Payload>("/api/missing-route", &[])
        .await
        .unwrap_err();
    match err {
        ClientError::Status { path, status } => {
            assert_eq!(path, "/api/missing-route");
            assert_eq!(status, 404);
        }
        other => panic!("expected Status, got {other:?}"),
    }
    // The 404 must not have triggered a second login.
    assert_eq!(stub.logins.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn get_json_returns_decode_when_the_body_does_not_match_the_wire_type() {
    let (client, _stub) = stub_client().await;
    #[derive(Debug, serde::Deserialize)]
    struct Wrong {
        #[allow(dead_code)]
        value: String, // stub serves a number
    }
    let err = client
        .get_json::<Wrong>("/api/thing", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Decode { .. }));
}

#[tokio::test]
async fn get_json_returns_http_when_the_instance_is_unreachable() {
    // Bind then drop a listener so the port is closed.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    let err = client
        .get_json::<Payload>("/api/thing", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Http(_)));
}

#[tokio::test]
async fn get_json_opt_maps_404_to_none_and_success_to_some() {
    let (client, _stub) = stub_client().await;
    let missing: Option<Payload> = client
        .get_json_opt("/api/missing-route", &[])
        .await
        .unwrap();
    assert_eq!(missing, None);
    let found: Option<Payload> = client.get_json_opt("/api/thing", &[]).await.unwrap();
    assert_eq!(found, Some(Payload { value: 42 }));
}

#[tokio::test]
async fn write_json_sends_the_body_with_bearer_and_decodes_the_response() {
    let (client, stub) = stub_client().await;
    let body = serde_json::json!({ "isbn": "9780306406157" });
    let answer: Payload = client
        .write_json(Method::POST, "/api/scan/resolve", &body)
        .await
        .unwrap();
    assert_eq!(answer, Payload { value: 42 });
    assert_eq!(stub.logins.load(Ordering::SeqCst), 1);
    let recorded = stub.last_write_body.lock().unwrap().clone().unwrap();
    assert_eq!(recorded, body);
}

#[tokio::test]
async fn write_json_relogs_in_transparently_when_the_session_expired() {
    let (client, stub) = stub_client().await;
    let body = serde_json::json!({ "isbn": "9780306406157" });
    let _: Payload = client
        .write_json(Method::POST, "/api/scan/resolve", &body)
        .await
        .unwrap();
    // Simulate the 7-day idle expiry: the server no longer knows the token.
    stub.revoke_all();
    let answer: Payload = client
        .write_json(Method::POST, "/api/scan/resolve", &body)
        .await
        .unwrap();
    assert_eq!(answer, Payload { value: 42 });
    assert_eq!(stub.logins.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn write_json_returns_write_status_carrying_the_server_message() {
    let (client, _stub) = stub_client().await;
    let body = serde_json::json!({ "query": "" });
    let err = client
        .write_json::<_, Payload>(Method::POST, "/api/scan/search", &body)
        .await
        .unwrap_err();
    match err {
        ClientError::WriteStatus {
            method,
            path,
            status,
            message,
        } => {
            assert_eq!(method, "POST");
            assert_eq!(path, "/api/scan/search");
            assert_eq!(status, 400);
            assert_eq!(message, "query is required");
        }
        other => panic!("expected WriteStatus, got {other:?}"),
    }
}

#[tokio::test]
async fn write_no_content_succeeds_on_a_204_delete() {
    let (client, stub) = stub_client().await;
    client
        .write_no_content(Method::DELETE, "/api/physical/uuid-1/wishlist")
        .await
        .unwrap();
    assert_eq!(stub.deletes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[should_panic(expected = "not in WRITE_ALLOWLIST")]
async fn write_json_panics_on_a_path_outside_the_allowlist() {
    let (client, _stub) = stub_client().await;
    let body = serde_json::json!({});
    let _: Result<Payload, _> = client
        .write_json(Method::POST, "/api/settings", &body)
        .await;
}

#[test]
fn write_allowlisted_expands_param_segments_and_rejects_everything_else() {
    // `{param}` matches exactly one non-empty segment.
    assert!(write_allowlisted(&Method::PATCH, "/api/physical/copies/12"));
    assert!(write_allowlisted(
        &Method::DELETE,
        "/api/physical/uuid-abc/wishlist"
    ));
    // Empty and extra segments do not satisfy a wildcard.
    assert!(!write_allowlisted(
        &Method::DELETE,
        "/api/physical//wishlist"
    ));
    assert!(!write_allowlisted(
        &Method::DELETE,
        "/api/physical/a/b/wishlist"
    ));
    // Method mismatch and unlisted paths are refused.
    assert!(!write_allowlisted(&Method::GET, "/api/scan/resolve"));
    assert!(!write_allowlisted(&Method::POST, "/api/settings"));
    assert!(!write_allowlisted(&Method::POST, "/api/scan/resolve/extra"));
}
