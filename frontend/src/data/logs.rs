//! Admin log-viewer data access.
//!
//! Web/SSR-only: the log viewer is not a mobile surface, so there is no
//! `reqwest`/REST counterpart — the whole module is gated off the mobile
//! build and calls the `rpc_get_logs` server function directly.
#![cfg(not(feature = "mobile"))]

use omnibus_shared::logs::{LogPage, LogQuery};

use super::{note_server_fn_err, DataError};

/// Fetch one filtered, newest-first page of on-disk log records. Admin-gated
/// server-side by the `rpc_get_logs` `AdminUser` extractor.
pub async fn get_logs(_server_url: &str, query: LogQuery) -> Result<LogPage, DataError> {
    crate::rpc::rpc_get_logs(query)
        .await
        .map_err(note_server_fn_err)
}
