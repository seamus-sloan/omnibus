//! Book / library / search / settings / overrides / worker fetchers.
//!
//! Each function has a mobile variant (REST via `reqwest`) and a web/SSR
//! variant (Dioxus server-function wrapper). The signatures are identical
//! across feature gates so call sites stay platform-agnostic.

use omnibus_shared::{
    EbookLibrary, EbookMetadata, LibraryContents, MetadataOverrides, PaletteResults, Settings,
    WorkerStatus,
};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

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
#[cfg(feature = "mobile")]
pub async fn get_library(server_url: &str) -> Result<LibraryContents, DataError> {
    let url = format!("{server_url}/api/library");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<LibraryContents>().await?)
}

/// GET `/api/ebooks` — fetch the full ebook library payload.
#[cfg(feature = "mobile")]
pub async fn get_ebooks(server_url: &str) -> Result<EbookLibrary, DataError> {
    let url = format!("{server_url}/api/ebooks");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<EbookLibrary>().await?)
}

/// GET `/api/search?q=` — full-text search across the ebook library.
#[cfg(feature = "mobile")]
pub async fn search_ebooks(server_url: &str, q: &str) -> Result<EbookLibrary, DataError> {
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
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<EbookLibrary>().await?)
}

/// Search palette — grouped results for the command-palette overlay (F1.5).
#[cfg(feature = "mobile")]
pub async fn search_palette(server_url: &str, q: &str) -> Result<PaletteResults, DataError> {
    let encoded: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{server_url}/api/search/palette?q={encoded}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<PaletteResults>().await?)
}

/// GET `/api/ebooks/{uuid}` — fetch one ebook by uuid, `Ok(None)` on 404.
#[cfg(feature = "mobile")]
pub async fn get_ebook(server_url: &str, uuid: &str) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// POST `/api/ebooks/{uuid}/overrides` — persist user metadata overrides.
#[cfg(feature = "mobile")]
pub async fn save_overrides(
    server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().post(&url))
        .json(overrides)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// DELETE `/api/ebooks/{uuid}/overrides` — revert to original metadata.
#[cfg(feature = "mobile")]
pub async fn delete_overrides(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

// ===== Web / fullstack-SSR transport: dioxus-fullstack server functions =====
//
// `server_url` is unused here — server functions always resolve against the
// page origin. We keep the parameter so the call sites stay platform-agnostic.

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

/// Snapshot of the worker progress feed. Web calls the RPC; mobile returns
/// an empty status because the corresponding REST endpoint doesn't exist
/// yet (issue #69 keeps the mobile UI out of scope for v1, and the data
/// stub keeps callers' types lined up across feature gates).
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

/// Web/SSR `get_ebooks` — server-function wrapper that proxies to `rpc_get_ebooks`.
#[cfg(not(feature = "mobile"))]
pub async fn get_ebooks(_server_url: &str) -> Result<EbookLibrary, DataError> {
    crate::rpc::rpc_get_ebooks()
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `search_ebooks` — server-function wrapper that proxies to `rpc_search`.
#[cfg(not(feature = "mobile"))]
pub async fn search_ebooks(_server_url: &str, q: &str) -> Result<EbookLibrary, DataError> {
    crate::rpc::rpc_search(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Search palette — grouped results for the command-palette overlay (F1.5).
#[cfg(not(feature = "mobile"))]
pub async fn search_palette(_server_url: &str, q: &str) -> Result<PaletteResults, DataError> {
    crate::rpc::rpc_search_palette(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_ebook` — server-function wrapper that proxies to `rpc_get_ebook`.
#[cfg(not(feature = "mobile"))]
pub async fn get_ebook(_server_url: &str, uuid: &str) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_get_ebook(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `save_overrides` — server-function wrapper that proxies to `rpc_save_overrides`.
#[cfg(not(feature = "mobile"))]
pub async fn save_overrides(
    _server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_save_overrides(uuid.to_string(), overrides.clone())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `delete_overrides` — server-function wrapper that proxies to `rpc_delete_overrides`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_overrides(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_delete_overrides(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
