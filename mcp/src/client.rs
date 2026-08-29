//! Authenticated HTTP client for one Omnibus instance. Logs in over
//! `POST /api/auth/login` for a bearer token, sends every read as a `GET`
//! with that token, and re-logs-in transparently when a session idle-expires
//! mid-run (7 days of inactivity — `SESSION_IDLE_TIMEOUT_SECS`). The password
//! and token are held in memory and never logged.

use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, RwLock};

use omnibus_shared::{LoginRequest, LoginResponse};

use crate::config::Config;

/// The complete set of mutating requests this crate is permitted to issue.
///
/// This is the write policy from `.claude/rules/08-offline-writes.md`, stated
/// once so later issues extend a list instead of inventing a policy: tools may
/// eventually write **content state** (progress, ratings, read status,
/// highlights, bookmarks, journals, shelf membership), and each such write
/// lands here as it ships. **Instance configuration** (`/api/settings`, API
/// keys, SMTP, registration) and **commands** (`/api/reindex`,
/// `/api/scan-library`, `/api/fts/rebuild`, `/api/kindle/send`) are never
/// added. Enforcement matches the statement: [`OmnibusClient`]'s public
/// surface builds only `GET` requests, and the one `POST` lives in the
/// private `login` path below.
pub const WRITE_ALLOWLIST: &[&str] = &["POST /api/auth/login"];

/// `User-Agent` on every request, so MCP traffic is separable from web and
/// iOS traffic in the instance's request log.
pub const USER_AGENT: &str = concat!("omnibus-mcp/", env!("CARGO_PKG_VERSION"));

/// `device_name` sent on login. Combined with `client_kind: "bearer"` the
/// server registers a device row, so MCP sessions carry a dedicated device in
/// `GET /api/auth/sessions` and the admin session views.
pub const DEVICE_NAME: &str = "omnibus-mcp";

/// Pagination metadata carried in response headers rather than the JSON body
/// (`X-Next-Cursor` / `X-Total-Count` on `/api/ebooks` and `/api/search`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageMeta {
    pub next_cursor: Option<String>,
    pub total: Option<i64>,
}

/// Why a request against the instance failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The login endpoint rejected the configured credentials (or errored).
    #[error("login failed: server answered HTTP {status}")]
    LoginFailed { status: u16 },
    /// Login succeeded but the response carried no bearer token — the server
    /// issued a cookie session, which this client cannot use.
    #[error("login succeeded but returned no bearer token")]
    NoToken,
    /// A read returned a non-success status other than the 401 the client
    /// already retried through.
    #[error("GET {path} failed: server answered HTTP {status}")]
    Status { path: String, status: u16 },
    /// The response body did not match the `omnibus_shared` wire type — the
    /// drift signal this crate exists to surface loudly.
    #[error("GET {path} did not match the expected wire shape: {message}")]
    Decode { path: String, message: String },
    /// Transport-level failure (connection refused, TLS, timeout, …).
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// One authenticated Omnibus instance. Cheap to share behind an `Arc`.
pub struct OmnibusClient {
    http: reqwest::Client,
    base_url: String,
    username: String,
    password: String,
    token: RwLock<Option<String>>,
    // Serializes logins so N racing tool calls produce one session, not N.
    login_lock: Mutex<()>,
}

impl OmnibusClient {
    /// Build a client for `config`'s instance. No network I/O — the first
    /// request performs the initial login lazily.
    pub fn new(config: Config) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self {
            http,
            base_url: config.base_url,
            username: config.username,
            password: config.password,
            token: RwLock::new(None),
            login_lock: Mutex::new(()),
        })
    }

    /// The one allowlisted write: `POST /api/auth/login` for a bearer
    /// session. Stores and returns the fresh token.
    async fn login(&self) -> Result<String, ClientError> {
        let body = LoginRequest {
            username: self.username.clone(),
            password: self.password.clone(),
            // `bearer` puts the token in the JSON body instead of a cookie;
            // together with `device_name` it registers a dedicated device row.
            client_kind: Some("bearer".to_string()),
            device_name: Some(DEVICE_NAME.to_string()),
            client_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        };
        let resp = self
            .http
            .post(format!("{}/api/auth/login", self.base_url))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(ClientError::LoginFailed {
                status: resp.status().as_u16(),
            });
        }
        let login: LoginResponse = resp.json().await?;
        let token = login.token.ok_or(ClientError::NoToken)?;
        *self.token.write().await = Some(token.clone());
        // Username only — never the password or token.
        tracing::info!(username = %self.username, "logged in to omnibus");
        Ok(token)
    }

    /// Log in unless another task already did while we waited for the
    /// lock: `stale` is the token the caller just saw rejected (`None`
    /// when it held none), so a held token that differs from it is fresh
    /// and is returned as-is. Concurrent tool calls racing from the same
    /// expired session thus perform one login, not one each.
    async fn login_once(&self, stale: Option<&str>) -> Result<String, ClientError> {
        let _guard = self.login_lock.lock().await;
        if let Some(current) = self.token.read().await.clone() {
            if stale != Some(current.as_str()) {
                return Ok(current);
            }
        }
        self.login().await
    }

    /// Current bearer token, logging in first if none is held yet.
    async fn bearer(&self) -> Result<String, ClientError> {
        if let Some(token) = self.token.read().await.clone() {
            return Ok(token);
        }
        self.login_once(None).await
    }

    async fn send_get(
        &self,
        path: &str,
        query: &[(&str, String)],
        token: &str,
    ) -> Result<reqwest::Response, ClientError> {
        Ok(self
            .http
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .bearer_auth(token)
            .send()
            .await?)
    }

    /// `GET path`, re-logging-in transparently on a 401: bearer sessions
    /// idle-expire after 7 days, so a long-idle MCP server's next tool call
    /// must re-establish the session without user intervention.
    async fn get_with_relogin(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<reqwest::Response, ClientError> {
        let token = self.bearer().await?;
        let resp = self.send_get(path, query, &token).await?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        let token = self.login_once(Some(&token)).await?;
        self.send_get(path, query, &token).await
    }

    /// `GET path` and decode the body as `T`, with the pagination headers.
    pub async fn get_json_with_meta<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<(T, PageMeta), ClientError> {
        let resp = self.get_with_relogin(path, query).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Status {
                path: path.to_string(),
                status: status.as_u16(),
            });
        }
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let meta = PageMeta {
            next_cursor: header("x-next-cursor"),
            total: header("x-total-count").and_then(|v| v.parse().ok()),
        };
        let bytes = resp.bytes().await?;
        let body = serde_json::from_slice(&bytes).map_err(|e| ClientError::Decode {
            path: path.to_string(),
            message: e.to_string(),
        })?;
        Ok((body, meta))
    }

    /// `GET path` and decode the body as `T`.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ClientError> {
        Ok(self.get_json_with_meta(path, query).await?.0)
    }

    /// `GET path`, mapping a 404 to `Ok(None)` — for the by-id endpoints
    /// where an unknown id is an answer, not a failure.
    pub async fn get_json_opt<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<T>, ClientError> {
        match self.get_json(path, query).await {
            Ok(body) => Ok(Some(body)),
            Err(ClientError::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests;
