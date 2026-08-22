//! Admin background-task history data access. Web/SSR-only, the same shape as
//! `errors` (the Server Health "Last errors" panel): there is no `reqwest` or
//! REST counterpart, so the whole module is gated off the mobile build and
//! calls the `rpc_get_background_tasks` server function directly.
#![cfg(not(feature = "mobile"))]

use omnibus_shared::BackgroundTaskRecord;

use super::{note_server_fn_err, DataError};

/// Fetch the most recent background-task history rows, newest first.
/// Admin-gated server-side by the `rpc_get_background_tasks` `AdminUser`
/// extractor.
pub async fn get_background_tasks() -> Result<Vec<BackgroundTaskRecord>, DataError> {
    crate::rpc::rpc_get_background_tasks()
        .await
        .map_err(note_server_fn_err)
}
