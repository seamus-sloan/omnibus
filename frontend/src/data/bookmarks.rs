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

/// Create a bookmark for the book with the given uuid. Offline, the
/// returned record carries a negative temp id that remaps on sync.
#[cfg(feature = "mobile")]
pub async fn create_bookmark(
    server_url: &str,
    input: CreateBookmark,
) -> Result<Bookmark, DataError> {
    let bookmark = crate::offline::sync::write_through(
        || create_bookmark_online(server_url, input.clone()),
        || crate::offline::outbox::queue_create_bookmark(&input),
    )
    .await?;
    crate::offline::outbox::apply::bookmark_created(&bookmark).await;
    Ok(bookmark)
}

#[cfg(feature = "mobile")]
pub(crate) async fn create_bookmark_online(
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
    crate::offline::cache::read_through(
        crate::offline::cache::keys::bookmarks(book_uuid),
        list_bookmarks_online(server_url, book_uuid),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn list_bookmarks_online(
    server_url: &str,
    book_uuid: &str,
) -> Result<Vec<Bookmark>, DataError> {
    let url = format!("{server_url}/api/bookmarks/book/{book_uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<Bookmark>>().await?)
}

/// Update a bookmark's title. Queued offline.
#[cfg(feature = "mobile")]
pub async fn update_bookmark(
    server_url: &str,
    id: i64,
    title: Option<String>,
) -> Result<(), DataError> {
    if id < 0 {
        crate::offline::outbox::queue_update_bookmark(id, title).await;
        return Ok(());
    }
    crate::offline::sync::write_through(
        || update_bookmark_online(server_url, id, title.clone()),
        || async {
            crate::offline::outbox::queue_update_bookmark(id, title.clone())
                .await
                .then_some(())
        },
    )
    .await?;
    // Keep the replica coherent when the write went straight to the server;
    // the queued path patches inside `queue_update_bookmark`.
    if let Some(book_uuid) = crate::offline::outbox::apply::find_bookmark_book(id).await {
        crate::offline::outbox::apply::bookmark_renamed(&book_uuid, id, title).await;
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) async fn update_bookmark_online(
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

/// Delete a bookmark. Deleting an unsynced (temp-id) bookmark cancels its
/// queued create with no server round trip.
#[cfg(feature = "mobile")]
pub async fn delete_bookmark(server_url: &str, id: i64) -> Result<(), DataError> {
    if id < 0 {
        crate::offline::outbox::queue_delete_bookmark(id).await;
        return Ok(());
    }
    let book_uuid = crate::offline::outbox::apply::find_bookmark_book(id).await;
    crate::offline::sync::write_through(
        || delete_bookmark_online(server_url, id),
        || async {
            crate::offline::outbox::queue_delete_bookmark(id)
                .await
                .then_some(())
        },
    )
    .await?;
    if let Some(book_uuid) = book_uuid {
        crate::offline::outbox::apply::bookmark_deleted(&book_uuid, id).await;
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) async fn delete_bookmark_online(server_url: &str, id: i64) -> Result<(), DataError> {
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
