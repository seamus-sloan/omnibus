//! Shelf transport. Wraps the rail list, detail, membership edits, and rule
//! preview for mobile (`/api/shelves*` via reqwest) and web/SSR (RPC server
//! functions in `crate::rpc`). Each function's public signature is shared
//! across platforms; the `#[cfg]` gates carry the split.

use omnibus_shared::{
    CreateShelfRequest, MatchMode, RulePreview, Shelf, ShelfPage, ShelfRule, ShelfSummary, SortDir,
    SortKey, UpdateShelfRequest,
};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

// Mobile (REST).

/// GET `/api/shelves` — the caller's visible shelves.
#[cfg(feature = "mobile")]
pub async fn list_shelves(server_url: &str) -> Result<Vec<ShelfSummary>, DataError> {
    let response = with_bearer(http_client().get(format!("{server_url}/api/shelves")))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<ShelfSummary>>().await?)
}

/// GET `/api/shelves/{id}` — one shelf's detail.
#[cfg(feature = "mobile")]
pub async fn get_shelf(server_url: &str, id: i64) -> Result<Shelf, DataError> {
    let response = with_bearer(http_client().get(format!("{server_url}/api/shelves/{id}")))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Shelf>().await?)
}

/// POST `/api/shelves` — create a shelf.
#[cfg(feature = "mobile")]
pub async fn create_shelf(server_url: &str, req: CreateShelfRequest) -> Result<Shelf, DataError> {
    let response = with_bearer(
        http_client()
            .post(format!("{server_url}/api/shelves"))
            .json(&req),
    )
    .send()
    .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Shelf>().await?)
}

/// PATCH `/api/shelves/{id}` — partial update.
#[cfg(feature = "mobile")]
pub async fn update_shelf(
    server_url: &str,
    id: i64,
    req: UpdateShelfRequest,
) -> Result<Shelf, DataError> {
    let response = with_bearer(
        http_client()
            .patch(format!("{server_url}/api/shelves/{id}"))
            .json(&req),
    )
    .send()
    .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Shelf>().await?)
}

/// DELETE `/api/shelves/{id}`.
#[cfg(feature = "mobile")]
pub async fn delete_shelf(server_url: &str, id: i64) -> Result<(), DataError> {
    let response = with_bearer(http_client().delete(format!("{server_url}/api/shelves/{id}")))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// GET `/api/shelves/{id}/page?sort=&dir=` — the shelf's member books.
#[cfg(feature = "mobile")]
pub async fn shelf_page(
    server_url: &str,
    id: i64,
    sort_key: SortKey,
    sort_dir: SortDir,
) -> Result<ShelfPage, DataError> {
    let url = format!(
        "{server_url}/api/shelves/{id}/page?sort={}&dir={}",
        sort_key.as_wire(),
        sort_dir.as_wire()
    );
    let response = with_bearer(http_client().get(url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<ShelfPage>().await?)
}

/// POST `/api/shelves/{id}/books` — append books to a hand-picked shelf.
#[cfg(feature = "mobile")]
pub async fn add_shelf_books(
    server_url: &str,
    id: i64,
    book_uuids: Vec<String>,
) -> Result<(), DataError> {
    let body = serde_json::json!({ "book_uuids": book_uuids });
    let response = with_bearer(
        http_client()
            .post(format!("{server_url}/api/shelves/{id}/books"))
            .json(&body),
    )
    .send()
    .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// DELETE `/api/shelves/{id}/books/{uuid}` — remove a book from a shelf.
#[cfg(feature = "mobile")]
pub async fn remove_shelf_book(
    server_url: &str,
    id: i64,
    book_uuid: &str,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/shelves/{id}/books/{book_uuid}");
    let response = with_bearer(http_client().delete(url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// POST `/api/shelves/preview` — evaluate an unsaved smart rule.
#[cfg(feature = "mobile")]
pub async fn preview_shelf_rule(
    server_url: &str,
    match_mode: MatchMode,
    rules: Vec<ShelfRule>,
) -> Result<RulePreview, DataError> {
    let body = serde_json::json!({ "match_mode": match_mode, "rules": rules });
    let response = with_bearer(
        http_client()
            .post(format!("{server_url}/api/shelves/preview"))
            .json(&body),
    )
    .send()
    .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<RulePreview>().await?)
}

// Web / SSR (RPC server functions).

/// The caller's visible shelves.
#[cfg(not(feature = "mobile"))]
pub async fn list_shelves(_server_url: &str) -> Result<Vec<ShelfSummary>, DataError> {
    crate::rpc::rpc_list_shelves()
        .await
        .map_err(note_server_fn_err)
}

/// One shelf's detail.
#[cfg(not(feature = "mobile"))]
pub async fn get_shelf(_server_url: &str, id: i64) -> Result<Shelf, DataError> {
    crate::rpc::rpc_get_shelf(id)
        .await
        .map_err(note_server_fn_err)
}

/// Create a shelf.
#[cfg(not(feature = "mobile"))]
pub async fn create_shelf(_server_url: &str, req: CreateShelfRequest) -> Result<Shelf, DataError> {
    crate::rpc::rpc_create_shelf(req)
        .await
        .map_err(note_server_fn_err)
}

/// Partially update a shelf.
#[cfg(not(feature = "mobile"))]
pub async fn update_shelf(
    _server_url: &str,
    id: i64,
    req: UpdateShelfRequest,
) -> Result<Shelf, DataError> {
    crate::rpc::rpc_update_shelf(id, req)
        .await
        .map_err(note_server_fn_err)
}

/// Delete a shelf.
#[cfg(not(feature = "mobile"))]
pub async fn delete_shelf(_server_url: &str, id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_delete_shelf(id)
        .await
        .map_err(note_server_fn_err)
}

/// The shelf's member books, sorted per `sort_key`/`sort_dir`.
#[cfg(not(feature = "mobile"))]
pub async fn shelf_page(
    _server_url: &str,
    id: i64,
    sort_key: SortKey,
    sort_dir: SortDir,
) -> Result<ShelfPage, DataError> {
    crate::rpc::rpc_get_shelf_page(id, sort_key, sort_dir)
        .await
        .map_err(note_server_fn_err)
}

/// Append books to a hand-picked shelf.
#[cfg(not(feature = "mobile"))]
pub async fn add_shelf_books(
    _server_url: &str,
    id: i64,
    book_uuids: Vec<String>,
) -> Result<(), DataError> {
    crate::rpc::rpc_add_shelf_books(id, book_uuids)
        .await
        .map_err(note_server_fn_err)
}

/// Remove a book from a shelf.
#[cfg(not(feature = "mobile"))]
pub async fn remove_shelf_book(
    _server_url: &str,
    id: i64,
    book_uuid: &str,
) -> Result<(), DataError> {
    crate::rpc::rpc_remove_shelf_book(id, book_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Evaluate an unsaved smart-shelf rule.
#[cfg(not(feature = "mobile"))]
pub async fn preview_shelf_rule(
    _server_url: &str,
    match_mode: MatchMode,
    rules: Vec<ShelfRule>,
) -> Result<RulePreview, DataError> {
    crate::rpc::rpc_preview_shelf_rule(match_mode, rules)
        .await
        .map_err(note_server_fn_err)
}
