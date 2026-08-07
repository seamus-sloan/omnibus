//! Settings, library-section, metadata-precedence, worker-status, and
//! scan/backfill fetchers — the admin/settings surface of the books domain.
//! Same mobile-REST vs web/SSR `#[cfg]` split as the sibling modules.

#[cfg(not(feature = "mobile"))]
use omnibus_shared::MetadataSource;
use omnibus_shared::{LibraryContents, Settings, WorkerStatus};

#[cfg(not(feature = "mobile"))]
use crate::data::note_server_fn_err;
use crate::data::DataError;
#[cfg(feature = "mobile")]
use crate::data::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/settings` — fetch library paths and indexer config.
#[cfg(feature = "mobile")]
pub async fn get_settings(server_url: &str) -> Result<Settings, DataError> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Settings>().await?)
}

/// POST `/api/settings` — persist updated library paths; server kicks a reindex.
#[cfg(feature = "mobile")]
pub async fn save_settings(server_url: &str, settings: Settings) -> Result<Settings, DataError> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().post(&url).json(&settings))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Settings>().await?)
}

/// GET `/api/library` — fetch the high-level library section listing.
/// Cache-first with background revalidation.
#[cfg(feature = "mobile")]
pub async fn get_library(server_url: &str) -> Result<LibraryContents, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through(crate::offline::cache::keys::library(), async move {
        get_library_online(&url).await
    })
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_library_online(server_url: &str) -> Result<LibraryContents, DataError> {
    let url = format!("{server_url}/api/library");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<LibraryContents>().await?)
}

/// Web/SSR `get_settings` — server-function wrapper that proxies to `rpc_get_settings`.
#[cfg(not(feature = "mobile"))]
pub async fn get_settings(_server_url: &str) -> Result<Settings, DataError> {
    crate::rpc::rpc_get_settings()
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `save_settings` — server-function wrapper that proxies to `rpc_save_settings`.
#[cfg(not(feature = "mobile"))]
pub async fn save_settings(_server_url: &str, settings: Settings) -> Result<Settings, DataError> {
    crate::rpc::rpc_save_settings(settings)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_library` — server-function wrapper that proxies to `rpc_get_library`.
#[cfg(not(feature = "mobile"))]
pub async fn get_library(_server_url: &str) -> Result<LibraryContents, DataError> {
    crate::rpc::rpc_get_library()
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR: fetch the metadata-source precedence for `library` (`"ebook"` or
/// `"audiobook"`, F5.1 #972) — server-function wrapper that proxies to
/// `rpc_get_metadata_precedence`. Web-only for now (no mobile REST route);
/// mobile keeps the today's-effective-order behavior until a mobile editing
/// surface is built.
#[cfg(not(feature = "mobile"))]
pub async fn get_metadata_precedence(
    _server_url: &str,
    library: &str,
) -> Result<Vec<MetadataSource>, DataError> {
    crate::rpc::rpc_get_metadata_precedence(library.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR: persist the metadata-source precedence for `library` — proxies
/// to `rpc_set_metadata_precedence`. Returns the saved order.
#[cfg(not(feature = "mobile"))]
pub async fn save_metadata_precedence(
    _server_url: &str,
    library: &str,
    precedence: Vec<MetadataSource>,
) -> Result<Vec<MetadataSource>, DataError> {
    crate::rpc::rpc_set_metadata_precedence(library.to_string(), precedence)
        .await
        .map_err(note_server_fn_err)
}

/// Snapshot of the worker progress feed. Web calls the RPC; mobile returns
/// an empty status because the corresponding REST endpoint doesn't exist
/// yet — the stub keeps callers' types lined up across feature gates.
#[cfg(not(feature = "mobile"))]
pub async fn worker_status(_server_url: &str) -> Result<WorkerStatus, DataError> {
    crate::rpc::rpc_worker_status()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `worker_status` — returns an empty snapshot until the REST mirror lands.
#[cfg(feature = "mobile")]
pub async fn worker_status(_server_url: &str) -> Result<WorkerStatus, DataError> {
    // Mobile REST mirror is a follow-up; return an empty status so any
    // future mobile caller compiles against the same signature the web
    // build uses.
    Ok(WorkerStatus::default())
}

/// Admin: manually trigger a rescan of the configured library paths.
#[cfg(not(feature = "mobile"))]
pub async fn scan_library(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_scan_library()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `scan_library` — queues both library scans via REST.
#[cfg(feature = "mobile")]
pub async fn scan_library(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/scan-library");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Admin: manually trigger chapter extraction for audiobooks missing chapters.
#[cfg(not(feature = "mobile"))]
pub async fn backfill_chapters(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_backfill_chapters()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `backfill_chapters`.
#[cfg(feature = "mobile")]
pub async fn backfill_chapters(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/audiobooks/backfill-chapters");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Admin: manually trigger a fleet-wide bake of every book's active
/// metadata/cover overrides into its EPUB container (#959, #1718). Queues
/// the run on the shared worker and returns as soon as it's queued;
/// completion surfaces via `worker_status` (the `WorkerStatusIndicator`).
#[cfg(not(feature = "mobile"))]
pub async fn rewrite_all_epubs(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_rewrite_all_epubs()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `rewrite_all_epubs` — hits the REST mirror directly.
#[cfg(feature = "mobile")]
pub async fn rewrite_all_epubs(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/admin/rewrite-all-epubs");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}
