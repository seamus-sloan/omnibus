//! Feature-gated data-fetching layer.
//!
//! Mobile calls the server's REST routes (`/api/*`) via `reqwest`, picking
//! up the base URL from the `ServerUrl` Dioxus context. Web will be swapped
//! to `#[server]` functions in Commit 4 — for now the web implementations
//! return placeholder defaults so the unified components compile and render
//! stubs while the transport migration is in progress.

use omnibus_shared::{LibraryContents, Settings};

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

// ===== Web transport: placeholder stubs (replaced with #[server] in Commit 4) =====

#[cfg(all(feature = "web", not(feature = "mobile")))]
pub async fn get_value(_server_url: &str) -> Result<i64, String> {
    Ok(0)
}

#[cfg(all(feature = "web", not(feature = "mobile")))]
pub async fn post_increment(_server_url: &str) -> Result<i64, String> {
    Ok(0)
}

#[cfg(all(feature = "web", not(feature = "mobile")))]
pub async fn get_settings(_server_url: &str) -> Result<Settings, String> {
    Ok(Settings::default())
}

#[cfg(all(feature = "web", not(feature = "mobile")))]
pub async fn save_settings(_server_url: &str, settings: Settings) -> Result<Settings, String> {
    Ok(settings)
}

#[cfg(all(feature = "web", not(feature = "mobile")))]
pub async fn get_library(_server_url: &str) -> Result<LibraryContents, String> {
    Ok(LibraryContents::default())
}
