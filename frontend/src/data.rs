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
