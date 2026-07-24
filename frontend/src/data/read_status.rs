//! Read/unread-state transport: mobile calls `/api/read-status*` via reqwest,
//! web/SSR the `crate::rpc` server functions, sharing each public signature so
//! callers stay platform-agnostic. Unlike ratings the mobile path talks to the
//! server directly (no offline outbox) — read state is a low-frequency toggle,
//! so a failed write surfaces as a revert rather than being queued.

use omnibus_shared::{ReadStatusRecord, SetReadStatus};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// PUT `/api/read-status` — set the current user's read state for a book.
#[cfg(feature = "mobile")]
pub async fn set_read_status(
    server_url: &str,
    update: SetReadStatus,
) -> Result<ReadStatusRecord, DataError> {
    let url = format!("{server_url}/api/read-status");
    let response = with_bearer(http_client().put(&url).json(&update))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<ReadStatusRecord>().await?)
}

/// GET `/api/read-status/{uuid}` — fetch the current read state, if any.
#[cfg(feature = "mobile")]
pub async fn get_read_status(
    server_url: &str,
    uuid: &str,
) -> Result<Option<ReadStatusRecord>, DataError> {
    let url = format!("{server_url}/api/read-status/{uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Option<ReadStatusRecord>>().await?)
}

/// Web/SSR `set_read_status` — server-function wrapper that proxies to
/// `rpc_set_read_status`.
#[cfg(not(feature = "mobile"))]
pub async fn set_read_status(
    _server_url: &str,
    update: SetReadStatus,
) -> Result<ReadStatusRecord, DataError> {
    crate::rpc::rpc_set_read_status(update)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_read_status` — server-function wrapper that proxies to
/// `rpc_get_read_status`.
#[cfg(not(feature = "mobile"))]
pub async fn get_read_status(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<ReadStatusRecord>, DataError> {
    crate::rpc::rpc_get_read_status(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
