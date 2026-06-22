//! Reading/listening progress-sync transport. Wraps `POST /api/progress`,
//! `GET /api/progress/{uuid}`, and `POST /api/progress/sessions` for
//! mobile, plus the matching `rpc_save_progress` / `rpc_get_progress` /
//! `rpc_record_sessions` server functions on the web/SSR path. Mobile and
//! web/SSR variants share each function's public signature so callers stay
//! platform-agnostic; the `#[cfg]` gates carry the split.

use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate, SessionReport};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// POST `/api/progress` — persist the latest reading/listening position.
#[cfg(feature = "mobile")]
pub async fn save_progress(
    server_url: &str,
    update: ProgressUpdate,
) -> Result<ProgressRecord, DataError> {
    let url = format!("{server_url}/api/progress");
    let response = with_bearer(http_client().post(&url).json(&update))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<ProgressRecord>().await?)
}

/// GET `/api/progress/{uuid}?format=…` — fetch the server-authoritative position, if any.
#[cfg(feature = "mobile")]
pub async fn get_progress(
    server_url: &str,
    uuid: &str,
    format: ProgressFormat,
) -> Result<Option<ProgressRecord>, DataError> {
    let fmt = match format {
        ProgressFormat::Epub => "epub",
        ProgressFormat::Audio => "audio",
    };
    let url = format!("{server_url}/api/progress/{uuid}?format={fmt}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Option<ProgressRecord>>().await?)
}

/// POST `/api/progress/sessions` — batched session-report ingest.
#[cfg(feature = "mobile")]
pub async fn record_sessions(
    server_url: &str,
    reports: Vec<SessionReport>,
) -> Result<u64, DataError> {
    let url = format!("{server_url}/api/progress/sessions");
    let response = with_bearer(http_client().post(&url).json(&reports))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    let body: serde_json::Value = response.json().await?;
    Ok(body.get("recorded").and_then(|v| v.as_u64()).unwrap_or(0))
}

/// Web/SSR `save_progress` — server-function wrapper that proxies to `rpc_save_progress`.
#[cfg(not(feature = "mobile"))]
pub async fn save_progress(
    _server_url: &str,
    update: ProgressUpdate,
) -> Result<ProgressRecord, DataError> {
    crate::rpc::rpc_save_progress(update)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_progress` — server-function wrapper that proxies to `rpc_get_progress`.
#[cfg(not(feature = "mobile"))]
pub async fn get_progress(
    _server_url: &str,
    uuid: &str,
    format: ProgressFormat,
) -> Result<Option<ProgressRecord>, DataError> {
    crate::rpc::rpc_get_progress(uuid.to_string(), format)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `record_sessions` — server-function wrapper that proxies to `rpc_record_sessions`.
#[cfg(not(feature = "mobile"))]
pub async fn record_sessions(
    _server_url: &str,
    reports: Vec<SessionReport>,
) -> Result<u64, DataError> {
    crate::rpc::rpc_record_sessions(reports)
        .await
        .map_err(note_server_fn_err)
}
