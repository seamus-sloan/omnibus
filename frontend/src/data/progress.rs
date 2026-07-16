//! Progress sync transport: `save_progress` / `get_progress` /
//! `record_sessions` over REST on mobile, plus matching `rpc_*` server
//! functions on web/SSR. Public signatures are identical across the
//! `#[cfg]` split so callers stay platform-agnostic.

use omnibus_shared::{
    AudiobookPlaybackRateRecord, AudiobookPlaybackRateUpdate, ProgressFormat, ProgressRecord,
    ProgressUpdate, ResumePoint, SessionReport,
};

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

/// PUT `/api/audiobooks/{uuid}/playback-rate` — persist a per-book rate.
#[cfg(feature = "mobile")]
pub async fn set_playback_rate(
    server_url: &str,
    uuid: &str,
    update: AudiobookPlaybackRateUpdate,
) -> Result<AudiobookPlaybackRateRecord, DataError> {
    let url = format!("{server_url}/api/audiobooks/{uuid}/playback-rate");
    let response = with_bearer(http_client().put(&url).json(&update))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<AudiobookPlaybackRateRecord>().await?)
}

/// GET `/api/audiobooks/{uuid}/playback-rate` — fetch a per-book rate.
#[cfg(feature = "mobile")]
pub async fn get_playback_rate(
    server_url: &str,
    uuid: &str,
) -> Result<Option<AudiobookPlaybackRateRecord>, DataError> {
    let url = format!("{server_url}/api/audiobooks/{uuid}/playback-rate");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response
        .json::<Option<AudiobookPlaybackRateRecord>>()
        .await?)
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

/// GET `/api/progress/recent?limit=…` — the "pick up where you left off" feed.
#[cfg(feature = "mobile")]
pub async fn recent_progress(server_url: &str, limit: i64) -> Result<Vec<ResumePoint>, DataError> {
    let url = format!("{server_url}/api/progress/recent?limit={limit}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<ResumePoint>>().await?)
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

/// Web/SSR playback-rate save through the matching server function.
#[cfg(not(feature = "mobile"))]
pub async fn set_playback_rate(
    _server_url: &str,
    uuid: &str,
    update: AudiobookPlaybackRateUpdate,
) -> Result<AudiobookPlaybackRateRecord, DataError> {
    crate::rpc::rpc_set_playback_rate(uuid.to_string(), update)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR playback-rate fetch through the matching server function.
#[cfg(not(feature = "mobile"))]
pub async fn get_playback_rate(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<AudiobookPlaybackRateRecord>, DataError> {
    crate::rpc::rpc_get_playback_rate(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `recent_progress` — server-function wrapper that proxies to `rpc_recent_progress`.
#[cfg(not(feature = "mobile"))]
pub async fn recent_progress(_server_url: &str, limit: i64) -> Result<Vec<ResumePoint>, DataError> {
    crate::rpc::rpc_recent_progress(limit)
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
