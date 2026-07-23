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
/// Queued offline; the drain's stale guard keeps a newer position written
/// by another device from being clobbered.
#[cfg(feature = "mobile")]
pub async fn save_progress(
    server_url: &str,
    update: ProgressUpdate,
) -> Result<ProgressRecord, DataError> {
    let record = crate::offline::sync::write_through(
        || save_progress_online(server_url, update.clone()),
        || crate::offline::outbox::queue_save_progress(&update),
    )
    .await?;
    crate::offline::outbox::apply::progress_saved(&record).await;
    Ok(record)
}

#[cfg(feature = "mobile")]
pub(crate) async fn save_progress_online(
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
/// Network-first with offline cache fallback.
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
    crate::offline::cache::read_through(
        crate::offline::cache::keys::progress(uuid, fmt),
        get_progress_online(server_url, uuid, format),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_progress_online(
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
/// Queued offline (coalesced per book, last write wins).
#[cfg(feature = "mobile")]
pub async fn set_playback_rate(
    server_url: &str,
    uuid: &str,
    update: AudiobookPlaybackRateUpdate,
) -> Result<AudiobookPlaybackRateRecord, DataError> {
    let record = crate::offline::sync::write_through(
        || set_playback_rate_online(server_url, uuid, update.clone()),
        || crate::offline::outbox::queue_set_playback_rate(uuid, &update),
    )
    .await?;
    crate::offline::cache::put_json(
        &crate::offline::cache::keys::playback_rate(uuid),
        &Some(record.clone()),
    );
    Ok(record)
}

#[cfg(feature = "mobile")]
pub(crate) async fn set_playback_rate_online(
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
/// Network-first with offline cache fallback.
#[cfg(feature = "mobile")]
pub async fn get_playback_rate(
    server_url: &str,
    uuid: &str,
) -> Result<Option<AudiobookPlaybackRateRecord>, DataError> {
    crate::offline::cache::read_through(
        crate::offline::cache::keys::playback_rate(uuid),
        get_playback_rate_online(server_url, uuid),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_playback_rate_online(
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
/// Queued offline so reading/listening time survives reconnect (F6.1 AC3).
#[cfg(feature = "mobile")]
pub async fn record_sessions(
    server_url: &str,
    reports: Vec<SessionReport>,
) -> Result<u64, DataError> {
    crate::offline::sync::write_through(
        || record_sessions_online(server_url, reports.clone()),
        || crate::offline::outbox::queue_record_sessions(&reports),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn record_sessions_online(
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
/// Network-first with offline cache fallback.
#[cfg(feature = "mobile")]
pub async fn recent_progress(server_url: &str, limit: i64) -> Result<Vec<ResumePoint>, DataError> {
    crate::offline::cache::read_through(
        format!("recent_progress:{limit}"),
        recent_progress_online(server_url, limit),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn recent_progress_online(
    server_url: &str,
    limit: i64,
) -> Result<Vec<ResumePoint>, DataError> {
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
