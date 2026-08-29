//! Hosted `/mcp` streamable-HTTP endpoint: rmcp's server transport mounted
//! on the fullstack router, authenticated by `Authorization: Bearer` (API
//! tokens are the documented credential; session bearers work but
//! idle-expire), gated per request on the admin `mcp_enabled` setting.
//! Tool calls run through the same `OmnibusMcp` tool layer as the stdio
//! binary, driving a loopback `OmnibusClient` against this server's own
//! REST surface with the caller's token passed through verbatim.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use omnibus_db::auth as auth_db;
use omnibus_mcp::client::OmnibusClient;
use omnibus_mcp::server::OmnibusMcp;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};

use crate::backend::AppState;
use crate::http_errors::internal;

tokio::task_local! {
    /// The caller's raw bearer for the request being served. rmcp's service
    /// factory takes no request context, so the handler scopes the token
    /// around `StreamableHttpService::handle` and the factory reads it back
    /// — the factory runs synchronously inside that scoped future, both for
    /// stateless per-request services and at legacy session creation.
    static CALLER_TOKEN: String;
}

/// Everything the `/mcp` handler needs: the app state (pool for the toggle
/// + auth checks) and the shared rmcp service.
#[derive(Clone)]
struct McpHttpState {
    app: AppState,
    service: Arc<StreamableHttpService<OmnibusMcp, LocalSessionManager>>,
}

/// Build the `/mcp` router. `loopback_url` is this server's own listen
/// address (`http://127.0.0.1:{port}`) — the embedded tool layer calls back
/// into the same process's REST surface, so every permission gate is
/// enforced by the same handlers the stdio transport hits remotely.
pub fn mcp_router(app: AppState, loopback_url: String) -> Router {
    // Host/Origin allowlisting is disabled: the stock allowlist admits only
    // localhost, which would reject every real deployment's public host.
    // DNS-rebinding defense is what that allowlist is for, and it doesn't
    // apply here — the endpoint is bearer-only, and a rebound browser
    // request cannot attach the caller's Authorization header.
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .disable_allowed_origins();
    let service = StreamableHttpService::new(
        move || {
            let token = CALLER_TOKEN
                .try_with(Clone::clone)
                .map_err(|_| std::io::Error::other("caller token not in scope"))?;
            let client = OmnibusClient::with_bearer(loopback_url.clone(), token)
                .map_err(std::io::Error::other)?;
            Ok(OmnibusMcp::new(Arc::new(client)))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let state = McpHttpState {
        app,
        service: Arc::new(service),
    };
    Router::new()
        .route("/mcp", any(mcp_handler))
        .with_state(state)
}

/// `ANY /mcp` — toggle gate, bearer auth, then hand the request to rmcp.
///
/// * Toggle read per request (AC5): flipping the admin setting takes effect
///   without a restart. Disabled → `404`, indistinguishable from a build
///   without the endpoint (AC2).
/// * Bearer-only auth: `resolve_token` routes `omni_…` API tokens to the
///   `api_tokens` table and session bearers to `sessions` — the same
///   routing every `/api/*` request gets, so a revoked token is rejected
///   here exactly as it is there (AC3 rides on the loopback REST calls).
async fn mcp_handler(State(state): State<McpHttpState>, req: Request) -> Response {
    match omnibus_db::mcp_enabled(state.app.pool()).await {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("read mcp_enabled", e),
    }

    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let Some(token) = bearer else {
        return unauthorized();
    };
    match auth_db::resolve_token(state.app.pool(), &token).await {
        Ok(_) => {}
        Err(auth_db::AuthError::SessionNotFound) => return unauthorized(),
        Err(e) => return internal("mcp auth", e),
    }

    let service = state.service.clone();
    let response = CALLER_TOKEN
        .scope(token, async move { service.handle(req).await })
        .await;
    response.map(Body::new)
}

/// 401 with a `WWW-Authenticate` challenge naming the expected scheme, so a
/// misconfigured MCP client surfaces a usable hint instead of a bare status.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        "unauthorized: pass an Omnibus API token as `Authorization: Bearer <token>`",
    )
        .into_response()
}

#[cfg(test)]
mod tests;
