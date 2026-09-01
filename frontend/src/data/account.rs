//! Account reading preferences: the hidden-formats list the caller's landing
//! view excludes, and whether their book detail page uses scroll stops.
//! Web/SSR goes through the server functions; mobile POSTs the REST routes
//! directly — account configuration is never queued in the outbox (rule 08),
//! so an offline save fails loudly rather than deferring.

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

/// Web/SSR: set the user's book-detail scroll-stops preference.
#[cfg(not(feature = "mobile"))]
pub async fn set_book_detail_scroll_stops(
    _server_url: &str,
    enabled: bool,
) -> Result<(), DataError> {
    crate::rpc::rpc_set_book_detail_scroll_stops(enabled)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/account/book-detail-scroll-stops`. Direct, like
/// `set_hidden_formats` above — account configuration, never queued.
#[cfg(feature = "mobile")]
pub async fn set_book_detail_scroll_stops(
    server_url: &str,
    enabled: bool,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/account/book-detail-scroll-stops");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "enabled": enabled }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}
