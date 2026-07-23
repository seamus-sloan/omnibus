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

/// Create a highlight for the book with the given uuid. Offline, the
/// returned record carries a negative temp id that remaps on sync.
#[cfg(feature = "mobile")]
pub async fn create_highlight(
    server_url: &str,
    input: CreateHighlight,
) -> Result<Highlight, DataError> {
    let highlight = crate::offline::sync::write_through(
        || create_highlight_online(server_url, input.clone()),
        || crate::offline::outbox::queue_create_highlight(&input),
    )
    .await?;
    crate::offline::outbox::apply::highlight_created(&highlight).await;
    Ok(highlight)
}

#[cfg(feature = "mobile")]
pub(crate) async fn create_highlight_online(
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

/// List highlights for the book with the given uuid.
#[cfg(feature = "mobile")]
pub async fn list_highlights(
    server_url: &str,
    book_uuid: &str,
) -> Result<Vec<Highlight>, DataError> {
    let url = server_url.to_string();
    let book_uuid = book_uuid.to_string();
    crate::offline::cache::read_through(
        crate::offline::cache::keys::highlights(&book_uuid),
        async move { list_highlights_online(&url, &book_uuid).await },
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn list_highlights_online(
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

/// Update a highlight's color. Queued offline (a temp-id edit rides on
/// the queued create's remap).
#[cfg(feature = "mobile")]
pub async fn update_highlight_color(
    server_url: &str,
    id: i64,
    color: HighlightColor,
) -> Result<(), DataError> {
    if id < 0 {
        // The highlight itself is still queued; patch the queued state only.
        crate::offline::outbox::queue_update_highlight_color(id, color).await;
        return Ok(());
    }
    crate::offline::sync::write_through(
        || update_highlight_color_online(server_url, id, color),
        || async {
            crate::offline::outbox::queue_update_highlight_color(id, color)
                .await
                .then_some(())
        },
    )
    .await?;
    // Keep the replica coherent when the write went straight to the server;
    // the queued path patches inside `queue_update_highlight_color`.
    if let Some(book_uuid) = crate::offline::outbox::apply::find_highlight_book(id).await {
        crate::offline::outbox::apply::highlight_color_changed(&book_uuid, id, color).await;
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) async fn update_highlight_color_online(
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

/// Update a highlight's note text. Queued offline.
#[cfg(feature = "mobile")]
pub async fn update_highlight_note(
    server_url: &str,
    id: i64,
    note: Option<String>,
) -> Result<(), DataError> {
    if id < 0 {
        crate::offline::outbox::queue_update_highlight_note(id, note).await;
        return Ok(());
    }
    crate::offline::sync::write_through(
        || update_highlight_note_online(server_url, id, note.clone()),
        || async {
            crate::offline::outbox::queue_update_highlight_note(id, note.clone())
                .await
                .then_some(())
        },
    )
    .await?;
    // Keep the replica coherent when the write went straight to the server;
    // the queued path patches inside `queue_update_highlight_note`.
    if let Some(book_uuid) = crate::offline::outbox::apply::find_highlight_book(id).await {
        crate::offline::outbox::apply::highlight_note_changed(&book_uuid, id, note).await;
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) async fn update_highlight_note_online(
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

/// Delete a highlight. Deleting an unsynced (temp-id) highlight cancels
/// its queued create with no server round trip.
#[cfg(feature = "mobile")]
pub async fn delete_highlight(server_url: &str, id: i64) -> Result<(), DataError> {
    if id < 0 {
        crate::offline::outbox::queue_delete_highlight(id).await;
        return Ok(());
    }
    let book_uuid = crate::offline::outbox::apply::find_highlight_book(id).await;
    crate::offline::sync::write_through(
        || delete_highlight_online(server_url, id),
        || async {
            crate::offline::outbox::queue_delete_highlight(id)
                .await
                .then_some(())
        },
    )
    .await?;
    if let Some(book_uuid) = book_uuid {
        crate::offline::outbox::apply::highlight_deleted(&book_uuid, id).await;
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) async fn delete_highlight_online(server_url: &str, id: i64) -> Result<(), DataError> {
    let url = format!("{server_url}/api/highlights/{id}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Create a highlight for the book with the given uuid.
#[cfg(not(feature = "mobile"))]
pub async fn create_highlight(
    _server_url: &str,
    input: CreateHighlight,
) -> Result<Highlight, DataError> {
    crate::rpc::rpc_create_highlight(input)
        .await
        .map_err(note_server_fn_err)
}

/// List highlights for the book with the given uuid.
#[cfg(not(feature = "mobile"))]
pub async fn list_highlights(
    _server_url: &str,
    book_uuid: &str,
) -> Result<Vec<Highlight>, DataError> {
    crate::rpc::rpc_list_highlights(book_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Update a highlight's color.
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

/// Update a highlight's note text.
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

/// Delete a highlight.
#[cfg(not(feature = "mobile"))]
pub async fn delete_highlight(_server_url: &str, id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_delete_highlight(id)
        .await
        .map_err(note_server_fn_err)
}
