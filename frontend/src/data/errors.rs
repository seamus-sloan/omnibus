//! Admin "last errors" data access.
//!
//! Web/SSR-only: the Server Health section is not a mobile surface, so
//! there is no `reqwest`/REST counterpart — the whole module is gated off
//! the mobile build and calls the `rpc_get_last_errors` server function
//! directly.
#![cfg(not(feature = "mobile"))]

use omnibus_shared::error_ring::CapturedError;

use super::{note_server_fn_err, DataError};

/// Fetch the in-memory error ring buffer's current contents, newest first.
/// Admin-gated server-side by the `rpc_get_last_errors` `AdminUser`
/// extractor.
pub async fn get_last_errors() -> Result<Vec<CapturedError>, DataError> {
    crate::rpc::rpc_get_last_errors()
        .await
        .map_err(note_server_fn_err)
}
