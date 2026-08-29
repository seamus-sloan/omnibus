//! Per-user API-token data access. Web/SSR-only — the management card lives
//! in the Settings → API Tokens section (no mobile surface), so there's no
//! `reqwest`/REST counterpart; it calls the `rpc_*_api_token` server
//! functions directly and is gated off the mobile build.
#![cfg(not(feature = "mobile"))]

use omnibus_shared::{ApiTokenView, CreateApiTokenResponse};

use super::{note_server_fn_err, DataError};

/// List the caller's live API tokens.
pub async fn list_api_tokens() -> Result<Vec<ApiTokenView>, DataError> {
    crate::rpc::rpc_list_api_tokens()
        .await
        .map_err(note_server_fn_err)
}

/// Mint a new API token; the response carries the secret exactly once.
pub async fn create_api_token(name: String) -> Result<CreateApiTokenResponse, DataError> {
    crate::rpc::rpc_create_api_token(name)
        .await
        .map_err(note_server_fn_err)
}

/// Revoke one of the caller's API tokens.
pub async fn revoke_api_token(id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_revoke_api_token(id)
        .await
        .map_err(note_server_fn_err)
}
