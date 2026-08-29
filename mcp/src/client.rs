//! Authenticated HTTP client for one Omnibus instance. Logs in over
//! `POST /api/auth/login` for a bearer token, sends reads as `GET`s and the
//! allowlisted writes with that token, and re-logs-in transparently when a
//! session idle-expires mid-run (7 days of inactivity —
//! `SESSION_IDLE_TIMEOUT_SECS`). The password and token are held in memory
//! and never logged.

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Serialize;
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
/// added. Enforcement matches the statement: [`OmnibusClient`]'s public read
/// surface builds only `GET` requests, and every non-`GET` routes through
/// [`OmnibusClient::write_json`] / [`OmnibusClient::write_no_content`], which
/// assert membership here (a `{param}` segment matches any single path
/// segment) — a request outside the list is a bug in this crate and fails
/// loudly before it reaches the instance.
pub const WRITE_ALLOWLIST: &[&str] = &[
    // The session itself (the one write the read-only crate shipped with).
    "POST /api/auth/login",
    // Reads wearing POST: the scan lookup ladder takes its argument in a
    // JSON body, but none of these three mutates anything — resolve answers
    // for one ISBN, search for a title query, resolve-meta for a picked
    // search candidate.
    "POST /api/scan/resolve",
    "POST /api/scan/search",
    "POST /api/scan/resolve-meta",
    // Content state (rule 08 tier 3): which physical copies the household
    // owns. Library-wide like a digital file, and an assertion ("we own this
    // copy"), not a command. Check-in also binds the ISBN to the book on the
    // exact-identifier rung, which is why the tool is confirm-gated.
    "POST /api/scan/check-in",
    "PATCH /api/physical/copies/{copy_id}",
    "DELETE /api/physical/copies/{copy_id}",
    // Content state (rule 08 tier 3): the caller's own wishlist rows. The
    // scan-flow add covers both a library book (by uuid) and a fileless book
    // from external meta; the remove is the per-book detail route.
    "POST /api/scan/wishlist",
    "DELETE /api/physical/{uuid}/wishlist",
    // Content state (rule 08 tier 3): which shelves exist for a user and
    // which books they hold is per-user content, not configuration.
    "POST /api/shelves",
    "PATCH /api/shelves/{id}",
    "DELETE /api/shelves/{id}",
    "POST /api/shelves/{id}/books",
    "DELETE /api/shelves/{id}/books/{uuid}",
    // Not a mutation at all — rule 08 calls this one out as "a read wearing
    // POST": it evaluates a candidate smart rule and creates nothing.
    "POST /api/shelves/preview",
    // Metadata overrides sit in rule 08's "library-wide, every user sees it"
    // tier — excluded from any offline queue for exactly that reason. They
    // are permitted here only as live, per-uuid, confirm-gated tool calls
    // (`apply_metadata_changes` / `revert_metadata_overrides` refuse without
    // an explicit `confirm: true`), and the server re-gates them on
    // `can_edit`.
    "POST /api/ebooks/{uuid}/overrides",
    "DELETE /api/ebooks/{uuid}/overrides",
    // Reads wearing POST (the `/api/shelves/preview` shape): they mutate
    // nothing, but spend outbound metadata-provider calls, so the server
    // gates them on `can_edit` like the overrides write.
    "POST /api/metadata/editions/search",
    "POST /api/metadata/editions/hydrate",
    // Book merge/undo — the strongest tier in this list: admin-gated on the
    // server, library-wide (merge deletes the source `books` row and
    // retargets every reader's state onto the target; undo restores it),
    // and confirm-gated in the tools (`merge_books` / `undo_merge` refuse
    // without an explicit `confirm: true`).
    "POST /api/books/merge",
    "POST /api/books/merge/undo",
];

/// True when `method path` is covered by [`WRITE_ALLOWLIST`]. A `{param}`
/// segment in an entry matches exactly one non-empty path segment.
fn write_allowlisted(method: &Method, path: &str) -> bool {
    WRITE_ALLOWLIST.iter().any(|entry| {
        entry
            .split_once(' ')
            .is_some_and(|(m, pattern)| m == method.as_str() && path_matches(pattern, path))
    })
}

/// Segment-wise match of a concrete request path against an allowlist
/// pattern, treating `{param}` segments as single-segment wildcards.
fn path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern = pattern.split('/');
    let mut path = path.split('/');
    loop {
        match (pattern.next(), path.next()) {
            (None, None) => return true,
            (Some(p), Some(s)) if p.starts_with('{') && p.ends_with('}') => {
                if s.is_empty() {
                    return false;
                }
            }
            (Some(p), Some(s)) if p == s => {}
            _ => return false,
        }
    }
}

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
    /// An allowlisted write (or a lookup the API models as `POST`) returned a
    /// non-success status. Carries the server's plain-text error body — the
    /// actionable detail (a 400's validation message, a 403's missing
    /// permission) that tools surface instead of an opaque status code.
    #[error("{method} {path} failed: server answered HTTP {status}: {message}")]
    WriteStatus {
        method: String,
        path: String,
        status: u16,
        message: String,
    },
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

    async fn send_write<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        token: &str,
    ) -> Result<reqwest::Response, ClientError> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(token);
        if let Some(body) = body {
            req = req.json(body);
        }
        Ok(req.send().await?)
    }

    /// `method path`, re-logging-in transparently on a 401 (same contract as
    /// [`Self::get_with_relogin`]). Panics when the request is not covered by
    /// [`WRITE_ALLOWLIST`] — that is a bug in this crate, and failing loudly
    /// beats letting an unvetted write reach the instance.
    async fn write_with_relogin<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<reqwest::Response, ClientError> {
        assert!(
            write_allowlisted(&method, path),
            "{method} {path} is not in WRITE_ALLOWLIST"
        );
        let token = self.bearer().await?;
        let resp = self.send_write(method.clone(), path, body, &token).await?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        let token = self.login_once(Some(&token)).await?;
        self.send_write(method, path, body, &token).await
    }

    /// Build the [`ClientError::WriteStatus`] for a failed write, carrying
    /// (a bounded prefix of) the server's plain-text error body.
    async fn write_status_error(
        method: &Method,
        path: &str,
        resp: reqwest::Response,
    ) -> ClientError {
        let status = resp.status().as_u16();
        let message: String = resp
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(500)
            .collect();
        ClientError::WriteStatus {
            method: method.to_string(),
            path: path.to_string(),
            status,
            message,
        }
    }

    /// Send an allowlisted write (with an optional JSON body) and decode the
    /// success response as `T`. Non-success statuses become
    /// [`ClientError::WriteStatus`] with the server's error body.
    pub async fn write_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let resp = self.write_with_relogin(method.clone(), path, body).await?;
        if !resp.status().is_success() {
            return Err(Self::write_status_error(&method, path, resp).await);
        }
        let bytes = resp.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Decode {
            path: path.to_string(),
            message: e.to_string(),
        })
    }

    /// Send an allowlisted write (with an optional JSON body) whose success
    /// answer carries no content — the `204` DELETE endpoints, and the shelf
    /// membership `POST` that acknowledges with `204`.
    pub async fn write_no_content<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), ClientError> {
        let resp = self.write_with_relogin(method.clone(), path, body).await?;
        if !resp.status().is_success() {
            return Err(Self::write_status_error(&method, path, resp).await);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
