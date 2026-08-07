//! Hidden-formats account preference: replace the caller's list of formats
//! their landing view excludes. Web/SSR goes through the server function;
//! mobile POSTs the REST route directly — account configuration is never
//! queued in the outbox (rule 08), so an offline save fails loudly rather
//! than deferring.

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// Web/SSR: replace the user's hidden-formats list (empty clears it).
#[cfg(not(feature = "mobile"))]
pub async fn set_hidden_formats(_server_url: &str, formats: Vec<String>) -> Result<(), DataError> {
    crate::rpc::rpc_set_hidden_formats(formats)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/account/hidden-formats`. Deliberately a direct call —
/// no `write_through`, no outbox op (do not copy `set_kindle_email`'s queued
/// shape; rule 08 flags it as the divergence).
#[cfg(feature = "mobile")]
pub async fn set_hidden_formats(server_url: &str, formats: Vec<String>) -> Result<(), DataError> {
    let url = format!("{server_url}/api/account/hidden-formats");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "formats": formats }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}
