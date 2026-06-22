//! Highlight annotation transport. Wraps the five highlight CRUD operations
//! for mobile (`/api/highlights/*` via reqwest) and web/SSR (RPC server
//! functions in `crate::rpc`). Mobile and web/SSR variants share each
//! function's public signature so callers stay platform-agnostic; the
//! `#[cfg]` gates carry the split.

use omnibus_shared::{CreateHighlight, Highlight, HighlightColor, UpdateHighlightNote};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

#[cfg(feature = "mobile")]
pub async fn create_highlight(
    server_url: &str,
    input: CreateHighlight,
) -> Result<Highlight, DataError> {
    let url = format!("{server_url}/api/highlights");
    let response = with_bearer(http_client().post(&url).json(&input))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Highlight>().await?)
}

#[cfg(feature = "mobile")]
pub async fn list_highlights(
    server_url: &str,
    book_uuid: &str,
) -> Result<Vec<Highlight>, DataError> {
    let url = format!("{server_url}/api/highlights/book/{book_uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<Highlight>>().await?)
}

#[cfg(feature = "mobile")]
pub async fn update_highlight_color(
    server_url: &str,
    id: i64,
    color: HighlightColor,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/highlights/{id}/color");
    let body = serde_json::json!({ "color": color });
    let response = with_bearer(http_client().patch(&url).json(&body))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub async fn update_highlight_note(
    server_url: &str,
    id: i64,
    note: Option<String>,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/highlights/{id}/note");
    let body = UpdateHighlightNote { note };
    let response = with_bearer(http_client().patch(&url).json(&body))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub async fn delete_highlight(server_url: &str, id: i64) -> Result<(), DataError> {
    let url = format!("{server_url}/api/highlights/{id}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

#[cfg(not(feature = "mobile"))]
pub async fn create_highlight(
    _server_url: &str,
    input: CreateHighlight,
) -> Result<Highlight, DataError> {
    crate::rpc::rpc_create_highlight(input)
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn list_highlights(
    _server_url: &str,
    book_uuid: &str,
) -> Result<Vec<Highlight>, DataError> {
    crate::rpc::rpc_list_highlights(book_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn update_highlight_color(
    _server_url: &str,
    id: i64,
    color: HighlightColor,
) -> Result<(), DataError> {
    crate::rpc::rpc_update_highlight_color(id, color)
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn update_highlight_note(
    _server_url: &str,
    id: i64,
    note: Option<String>,
) -> Result<(), DataError> {
    crate::rpc::rpc_update_highlight_note(id, UpdateHighlightNote { note })
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn delete_highlight(_server_url: &str, id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_delete_highlight(id)
        .await
        .map_err(note_server_fn_err)
}
