//! Physical-collection + wishlist transport for the book-detail page: copies
//! (list / edit-note / delete), wishlist entry (get / add / remove), and
//! fileless-book removal. Mobile calls `/api/physical/*` REST routes; web/SSR
//! proxy the RPC server functions in `crate::rpc` behind a shared signature.

use omnibus_shared::physical::{PhysicalCopy, WishlistEntry};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};
#[cfg(feature = "mobile")]
use omnibus_shared::physical::UpdateCopyNoteRequest;

// Mobile (REST).

/// GET `/api/physical/{uuid}/copies` — a book's physical copies, oldest first.
#[cfg(feature = "mobile")]
pub async fn list_physical_copies(
    server_url: &str,
    uuid: &str,
) -> Result<Vec<PhysicalCopy>, DataError> {
    let url = format!("{server_url}/api/physical/{uuid}/copies");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<PhysicalCopy>>().await?)
}

/// PATCH `/api/physical/copies/{copy_id}` — replace a copy's note (blank clears).
#[cfg(feature = "mobile")]
pub async fn update_physical_copy_note(
    server_url: &str,
    copy_id: i64,
    note: Option<String>,
) -> Result<PhysicalCopy, DataError> {
    let url = format!("{server_url}/api/physical/copies/{copy_id}");
    let body = UpdateCopyNoteRequest { note };
    let response = with_bearer(http_client().patch(&url).json(&body))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<PhysicalCopy>().await?)
}

/// DELETE `/api/physical/copies/{copy_id}` — delete one copy ("I sold it").
#[cfg(feature = "mobile")]
pub async fn delete_physical_copy(server_url: &str, copy_id: i64) -> Result<(), DataError> {
    let url = format!("{server_url}/api/physical/copies/{copy_id}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// GET `/api/physical/{uuid}/wishlist` — the caller's wishlist entry, or `None`.
#[cfg(feature = "mobile")]
pub async fn get_wishlist_entry(
    server_url: &str,
    uuid: &str,
) -> Result<Option<WishlistEntry>, DataError> {
    let url = format!("{server_url}/api/physical/{uuid}/wishlist");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Option<WishlistEntry>>().await?)
}

/// POST `/api/physical/{uuid}/wishlist` — add a book to the caller's wishlist.
#[cfg(feature = "mobile")]
pub async fn add_wishlist_entry(server_url: &str, uuid: &str) -> Result<WishlistEntry, DataError> {
    let url = format!("{server_url}/api/physical/{uuid}/wishlist");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<WishlistEntry>().await?)
}

/// DELETE `/api/physical/{uuid}/wishlist` — remove the caller's wishlist entry.
#[cfg(feature = "mobile")]
pub async fn remove_wishlist_entry(server_url: &str, uuid: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/physical/{uuid}/wishlist");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// DELETE `/api/physical/{uuid}` — remove a fileless book outright.
#[cfg(feature = "mobile")]
pub async fn delete_fileless_book(server_url: &str, uuid: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/physical/{uuid}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

// Web (RPC).

/// Web/SSR `list_physical_copies` — proxies to `rpc_list_physical_copies`.
#[cfg(not(feature = "mobile"))]
pub async fn list_physical_copies(
    _server_url: &str,
    uuid: &str,
) -> Result<Vec<PhysicalCopy>, DataError> {
    crate::rpc::rpc_list_physical_copies(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `update_physical_copy_note` — proxies to `rpc_update_physical_copy_note`.
#[cfg(not(feature = "mobile"))]
pub async fn update_physical_copy_note(
    _server_url: &str,
    copy_id: i64,
    note: Option<String>,
) -> Result<PhysicalCopy, DataError> {
    crate::rpc::rpc_update_physical_copy_note(copy_id, note)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `delete_physical_copy` — proxies to `rpc_delete_physical_copy`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_physical_copy(_server_url: &str, copy_id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_delete_physical_copy(copy_id)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_wishlist_entry` — proxies to `rpc_get_wishlist_entry`.
#[cfg(not(feature = "mobile"))]
pub async fn get_wishlist_entry(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<WishlistEntry>, DataError> {
    crate::rpc::rpc_get_wishlist_entry(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `add_wishlist_entry` — proxies to `rpc_add_wishlist_entry`.
#[cfg(not(feature = "mobile"))]
pub async fn add_wishlist_entry(_server_url: &str, uuid: &str) -> Result<WishlistEntry, DataError> {
    crate::rpc::rpc_add_wishlist_entry(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `remove_wishlist_entry` — proxies to `rpc_remove_wishlist_entry`.
#[cfg(not(feature = "mobile"))]
pub async fn remove_wishlist_entry(_server_url: &str, uuid: &str) -> Result<(), DataError> {
    crate::rpc::rpc_remove_wishlist_entry(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `delete_fileless_book` — proxies to `rpc_delete_fileless_book`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_fileless_book(_server_url: &str, uuid: &str) -> Result<(), DataError> {
    crate::rpc::rpc_delete_fileless_book(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
