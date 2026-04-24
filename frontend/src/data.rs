//! Feature-gated data-fetching layer.
//!
//! - Mobile (`feature = "mobile"`) calls the server's hand-written REST
//!   routes (`/api/*`) via `reqwest`, picking up the base URL from the
//!   `ServerUrl` Dioxus context. `server_url` is required at the call site.
//! - Web (`feature = "web"`) calls the `#[get]`/`#[post]` server functions
//!   defined in [`crate::rpc`]. No base URL needed — the server-function
//!   client stubs use the page origin automatically. `server_url` is
//!   ignored on the web path.
//! - Server-only compiles (`feature = "server"` without `"web"`) reuse the
//!   web stubs so SSR-during-fullstack-render still returns sensible data.

use omnibus_shared::{EbookLibrary, LibraryContents, Settings};
#[cfg(feature = "web")]
use omnibus_shared::{LoginRequest, LoginResponse, RegisterRequest, UserSummary};

// ===== Mobile transport: reqwest =====

#[cfg(feature = "mobile")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerUrl(pub String);

#[cfg(feature = "mobile")]
pub async fn get_value(server_url: &str) -> Result<i64, String> {
    let url = format!("{server_url}/api/value");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    payload["value"]
        .as_i64()
        .ok_or_else(|| "missing value field".into())
}

#[cfg(feature = "mobile")]
pub async fn post_increment(server_url: &str) -> Result<i64, String> {
    let url = format!("{server_url}/api/value/increment");
    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    payload["value"]
        .as_i64()
        .ok_or_else(|| "missing value field".into())
}

#[cfg(feature = "mobile")]
pub async fn get_settings(server_url: &str) -> Result<Settings, String> {
    let url = format!("{server_url}/api/settings");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    response.json::<Settings>().await.map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn save_settings(server_url: &str, settings: Settings) -> Result<Settings, String> {
    let url = format!("{server_url}/api/settings");
    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .json(&settings)
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    if !response.status().is_success() {
        return Err(format!("Server error: {}", response.status()));
    }
    response.json::<Settings>().await.map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn get_library(server_url: &str) -> Result<LibraryContents, String> {
    let url = format!("{server_url}/api/library");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    response
        .json::<LibraryContents>()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn get_ebooks(server_url: &str) -> Result<EbookLibrary, String> {
    let url = format!("{server_url}/api/ebooks");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server error {status}: {body}"));
    }
    response
        .json::<EbookLibrary>()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn search_ebooks(server_url: &str, q: &str) -> Result<EbookLibrary, String> {
    // Percent-encode the query so FTS5 operators and whitespace survive the
    // URL.
    let encoded: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{server_url}/api/search?q={encoded}");
    let response = reqwest::get(&url).await.map_err(|e| format!("{e:#}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Server error {status}: {body}"));
    }
    response
        .json::<EbookLibrary>()
        .await
        .map_err(|e| e.to_string())
}

// ===== Web / fullstack-SSR transport: dioxus-fullstack server functions =====
//
// `server_url` is unused here — server functions always resolve against the
// page origin. We keep the parameter so the call sites stay platform-agnostic.

#[cfg(not(feature = "mobile"))]
pub async fn get_value(_server_url: &str) -> Result<i64, String> {
    match crate::rpc::rpc_get_value().await {
        Ok(payload) => Ok(payload.value),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(feature = "mobile"))]
pub async fn post_increment(_server_url: &str) -> Result<i64, String> {
    match crate::rpc::rpc_increment_value().await {
        Ok(payload) => Ok(payload.value),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(feature = "mobile"))]
pub async fn get_settings(_server_url: &str) -> Result<Settings, String> {
    crate::rpc::rpc_get_settings()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn save_settings(_server_url: &str, settings: Settings) -> Result<Settings, String> {
    crate::rpc::rpc_save_settings(settings)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn get_library(_server_url: &str) -> Result<LibraryContents, String> {
    crate::rpc::rpc_get_library()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn get_ebooks(_server_url: &str) -> Result<EbookLibrary, String> {
    crate::rpc::rpc_get_ebooks()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn search_ebooks(_server_url: &str, q: &str) -> Result<EbookLibrary, String> {
    crate::rpc::rpc_search(q.to_string())
        .await
        .map_err(|e| e.to_string())
}

// ===== Auth transport (web only) =====
//
// The web client hits the REST auth endpoints directly via `gloo-net` rather
// than going through a Dioxus server function. The REST endpoints already
// know how to set/clear the `omnibus_session` cookie via the `CookieJar`
// extractor, and browser fetch (same-origin) round-trips the cookie
// automatically. Server functions would force us to re-plumb cookie
// handling through the Dioxus fullstack response shape for no gain.
//
// SSR and server-only builds don't need these helpers: the login/register
// pages render the same markup on the server (no auth calls issued during
// SSR), and the actions only fire on user interaction after hydration.

#[cfg(feature = "web")]
pub async fn login(req: LoginRequest) -> Result<LoginResponse, String> {
    post_auth_json("/api/auth/login", &req).await
}

#[cfg(feature = "web")]
pub async fn register(req: RegisterRequest) -> Result<LoginResponse, String> {
    post_auth_json("/api/auth/register", &req).await
}

#[cfg(feature = "web")]
pub async fn logout() -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::post("/api/auth/logout")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() && res.status() != 204 {
        return Err(format!("logout failed: {}", res.status()));
    }
    Ok(())
}

#[cfg(feature = "web")]
pub async fn current_user() -> Result<Option<UserSummary>, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/auth/me")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status() == 401 {
        return Ok(None);
    }
    if !res.ok() {
        return Err(format!("me failed: {}", res.status()));
    }
    res.json::<UserSummary>()
        .await
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "web")]
async fn post_auth_json<T: serde::Serialize>(
    path: &str,
    body: &T,
) -> Result<LoginResponse, String> {
    use gloo_net::http::Request;
    let res = Request::post(path)
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        let status = res.status();
        let msg = res.text().await.unwrap_or_default();
        return Err(format!("{status}: {msg}"));
    }
    res.json::<LoginResponse>().await.map_err(|e| e.to_string())
}
