//! Suggestions fetchers + Hardcover-key read/write. Each function has a
//! mobile REST variant (`reqwest`) and a web/SSR server-function wrapper with
//! identical signatures across the `#[cfg]` split.

use omnibus_shared::{HardcoverKeyStatus, SuggestionsResponse};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

// ── Suggestions strip ────────────────────────────────────────────

/// Web/SSR: fetch a book's "readers also enjoyed" strip via `rpc_get_suggestions`.
#[cfg(not(feature = "mobile"))]
pub async fn get_suggestions(
    _server_url: &str,
    uuid: &str,
) -> Result<SuggestionsResponse, DataError> {
    crate::rpc::rpc_get_suggestions(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: GET `/api/ebooks/{uuid}/suggestions`.
#[cfg(feature = "mobile")]
pub async fn get_suggestions(
    server_url: &str,
    uuid: &str,
) -> Result<SuggestionsResponse, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/suggestions");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<SuggestionsResponse>().await?)
}

// ── Hardcover key (admin settings) ───────────────────────────────

/// Web/SSR: read the masked Hardcover key status via `rpc_get_hardcover_key`.
#[cfg(not(feature = "mobile"))]
pub async fn get_hardcover_key(_server_url: &str) -> Result<HardcoverKeyStatus, DataError> {
    crate::rpc::rpc_get_hardcover_key()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: GET `/api/hardcover-key`.
#[cfg(feature = "mobile")]
pub async fn get_hardcover_key(server_url: &str) -> Result<HardcoverKeyStatus, DataError> {
    let url = format!("{server_url}/api/hardcover-key");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<HardcoverKeyStatus>().await?)
}

/// Web/SSR: save (or clear, with `None`) the Hardcover key via
/// `rpc_set_hardcover_key`. Returns the new masked status.
#[cfg(not(feature = "mobile"))]
pub async fn set_hardcover_key(
    _server_url: &str,
    key: Option<String>,
) -> Result<HardcoverKeyStatus, DataError> {
    crate::rpc::rpc_set_hardcover_key(key)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/hardcover-key` with `{ "key": ... }`.
#[cfg(feature = "mobile")]
pub async fn set_hardcover_key(
    server_url: &str,
    key: Option<String>,
) -> Result<HardcoverKeyStatus, DataError> {
    let url = format!("{server_url}/api/hardcover-key");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "key": key }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<HardcoverKeyStatus>().await?)
}
