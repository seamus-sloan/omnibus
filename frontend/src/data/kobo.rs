//! Per-user Kobo device data access. Web/SSR-only — the wireless-sync device
//! manager lives in the Account settings card (no mobile surface), so there's
//! no `reqwest`/REST counterpart; it calls the `rpc_*_kobo_device` server
//! functions directly and is gated off the mobile build.
#![cfg(not(feature = "mobile"))]

use omnibus_shared::KoboDeviceView;

use super::{note_server_fn_err, DataError};

/// List the caller's registered Kobo devices.
pub async fn list_kobo_devices(_server_url: &str) -> Result<Vec<KoboDeviceView>, DataError> {
    crate::rpc::rpc_list_kobo_devices()
        .await
        .map_err(note_server_fn_err)
}

/// Register a new Kobo, returning the created device (with its fresh token).
pub async fn create_kobo_device(
    _server_url: &str,
    name: String,
) -> Result<KoboDeviceView, DataError> {
    crate::rpc::rpc_create_kobo_device(name)
        .await
        .map_err(note_server_fn_err)
}

/// Rotate a device's token, returning the updated device (with the new token).
pub async fn regenerate_kobo_device(
    _server_url: &str,
    id: i64,
) -> Result<KoboDeviceView, DataError> {
    crate::rpc::rpc_regenerate_kobo_device(id)
        .await
        .map_err(note_server_fn_err)
}

/// Remove a registered Kobo, revoking its token.
pub async fn revoke_kobo_device(_server_url: &str, id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_revoke_kobo_device(id)
        .await
        .map_err(note_server_fn_err)
}

/// Read the caller's annotation down-sync opt-in.
pub async fn kobo_annotation_sync(_server_url: &str) -> Result<bool, DataError> {
    crate::rpc::rpc_kobo_annotation_sync()
        .await
        .map_err(note_server_fn_err)
}

/// Set the caller's annotation down-sync opt-in.
pub async fn set_kobo_annotation_sync(_server_url: &str, enabled: bool) -> Result<(), DataError> {
    crate::rpc::rpc_set_kobo_annotation_sync(enabled)
        .await
        .map_err(note_server_fn_err)
}
