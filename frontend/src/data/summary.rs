//! On-demand summary-fetch wrappers for the "Fetch Summary" button. Each
//! function has a mobile REST variant (`reqwest`) and a web/SSR
//! server-function wrapper with identical signatures across the `#[cfg]`
//! split.

use omnibus_shared::summary::SummarySource;

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// Web/SSR: fetch a summary for `uuid` from `source` via `rpc_fetch_summary`.
/// `Ok(None)` when that source had no summary (the caller cascades to the next).
#[cfg(not(feature = "mobile"))]
pub async fn fetch_summary(
    _server_url: &str,
    uuid: &str,
    source: SummarySource,
) -> Result<Option<String>, DataError> {
    crate::rpc::rpc_fetch_summary(uuid.to_string(), source)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR: whether a server-wide Hardcover key is configured — drives whether
/// the client's cascade starts at Hardcover or goes straight to OpenLibrary.
#[cfg(not(feature = "mobile"))]
pub async fn hardcover_configured(_server_url: &str) -> Result<bool, DataError> {
    crate::rpc::rpc_hardcover_configured()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/ebooks/{uuid}/summary/fetch` with `{ "source": ... }`.
/// `Ok(None)` when that source had no summary (the caller cascades to the next).
#[cfg(feature = "mobile")]
pub async fn fetch_summary(
    server_url: &str,
    uuid: &str,
    source: SummarySource,
) -> Result<Option<String>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/summary/fetch");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "source": source }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Option<String>>().await?)
}

/// Mobile: GET `/api/summary/hardcover-configured` — see the web
/// `hardcover_configured` doc.
#[cfg(feature = "mobile")]
pub async fn hardcover_configured(server_url: &str) -> Result<bool, DataError> {
    let url = format!("{server_url}/api/summary/hardcover-configured");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<bool>().await?)
}
