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

/// Web/SSR: the ordered summary-source cascade for the current settings —
/// drives which sources the client attempts, and in what order.
#[cfg(not(feature = "mobile"))]
pub async fn summary_sources(_server_url: &str) -> Result<Vec<SummarySource>, DataError> {
    crate::rpc::rpc_summary_sources()
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

/// Mobile: GET `/api/summary/sources` — see the web `summary_sources` doc.
#[cfg(feature = "mobile")]
pub async fn summary_sources(server_url: &str) -> Result<Vec<SummarySource>, DataError> {
    let url = format!("{server_url}/api/summary/sources");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<SummarySource>>().await?)
}
