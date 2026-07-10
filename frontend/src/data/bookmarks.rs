//! Bookmark transport. Wraps the four bookmark CRUD operations for mobile
//! (`/api/bookmarks/*` via reqwest) and web/SSR (RPC server functions in
//! `crate::rpc`). Mobile and web/SSR variants share each function's public
//! signature so callers stay platform-agnostic; the `#[cfg]` gates carry the
//! split. One model serves the audiobook player and the reader.

use omnibus_shared::{Bookmark, CreateBookmark, UpdateBookmark};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// Create a bookmark for the book with the given uuid.
#[cfg(feature = "mobile")]
pub async fn create_bookmark(
    server_url: &str,
    input: CreateBookmark,
) -> Result<Bookmark, DataError> {
    let url = format!("{server_url}/api/bookmarks");
    let response = with_bearer(http_client().post(&url).json(&input))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Bookmark>().await?)
}

/// List bookmarks for the book with the given uuid.
#[cfg(feature = "mobile")]
pub async fn list_bookmarks(server_url: &str, book_uuid: &str) -> Result<Vec<Bookmark>, DataError> {
    let url = format!("{server_url}/api/bookmarks/book/{book_uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<Bookmark>>().await?)
}

/// Update a bookmark's title.
#[cfg(feature = "mobile")]
pub async fn update_bookmark(
    server_url: &str,
    id: i64,
    title: Option<String>,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/bookmarks/{id}");
    let body = UpdateBookmark { title };
    let response = with_bearer(http_client().put(&url).json(&body))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Delete a bookmark.
#[cfg(feature = "mobile")]
pub async fn delete_bookmark(server_url: &str, id: i64) -> Result<(), DataError> {
    let url = format!("{server_url}/api/bookmarks/{id}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Create a bookmark for the book with the given uuid.
#[cfg(not(feature = "mobile"))]
pub async fn create_bookmark(
    _server_url: &str,
    input: CreateBookmark,
) -> Result<Bookmark, DataError> {
    crate::rpc::rpc_create_bookmark(input)
        .await
        .map_err(note_server_fn_err)
}

/// List bookmarks for the book with the given uuid.
#[cfg(not(feature = "mobile"))]
pub async fn list_bookmarks(
    _server_url: &str,
    book_uuid: &str,
) -> Result<Vec<Bookmark>, DataError> {
    crate::rpc::rpc_list_bookmarks(book_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Update a bookmark's title.
#[cfg(not(feature = "mobile"))]
pub async fn update_bookmark(
    _server_url: &str,
    id: i64,
    title: Option<String>,
) -> Result<(), DataError> {
    crate::rpc::rpc_update_bookmark(id, UpdateBookmark { title })
        .await
        .map_err(note_server_fn_err)
}

/// Delete a bookmark.
#[cfg(not(feature = "mobile"))]
pub async fn delete_bookmark(_server_url: &str, id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_delete_bookmark(id)
        .await
        .map_err(note_server_fn_err)
}
