//! Author CRUD + author-photo upload/scan helpers.
//!
//! Includes the multipart photo-upload path which bypasses the
//! server-function transport on web (server functions can't carry binary
//! payloads) and posts directly to the REST endpoint via `gloo-net`.

use omnibus_shared::{AuthorDetail, AuthorPhotoScanResult, AuthorSummary};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/authors/{id}` — fetch one author detail, `Ok(None)` on 404.
#[cfg(feature = "mobile")]
pub async fn get_author(server_url: &str, id: i64) -> Result<Option<AuthorDetail>, DataError> {
    let url = format!("{server_url}/api/authors/{id}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<AuthorDetail>().await?))
}

/// Persist an author photo by URL. Server fetches and validates the URL —
/// see `db::author_photos::fetch_remote_image`.
#[cfg(feature = "mobile")]
pub async fn set_author_photo_url(server_url: &str, id: i64, url: String) -> Result<(), DataError> {
    let endpoint = format!("{server_url}/api/authors/{id}/photo/url");
    let response = with_bearer(http_client().put(&endpoint))
        .json(&serde_json::json!({ "url": url }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Multipart upload of an author photo. Mobile mirrors the web FormData
/// path — the same `/api/authors/:id/photo` PUT endpoint.
#[cfg(feature = "mobile")]
pub async fn upload_author_photo(
    server_url: &str,
    id: i64,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<(), DataError> {
    let endpoint = format!("{server_url}/api/authors/{id}/photo");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime)?;
    let form = reqwest::multipart::Form::new().part("photo", part);
    let response = with_bearer(http_client().put(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Admin "Scan for picture" — synchronously re-runs the Open Library
/// cascade and returns whether a photo was found.
#[cfg(feature = "mobile")]
pub async fn scan_author_photo(
    server_url: &str,
    id: i64,
) -> Result<AuthorPhotoScanResult, DataError> {
    let url = format!("{server_url}/api/authors/{id}/photo/scan");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<AuthorPhotoScanResult>().await?)
}

/// Admin: bulk re-resolve all author photos via the background worker.
#[cfg(feature = "mobile")]
pub async fn refetch_author_photos(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/authors/refetch-photos");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// GET `/api/authors` — fetch the full authors index for browse / autocomplete.
#[cfg(feature = "mobile")]
pub async fn list_authors(server_url: &str) -> Result<Vec<AuthorSummary>, DataError> {
    let url = format!("{server_url}/api/authors");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<AuthorSummary>>().await?)
}

/// Web/SSR `get_author` — server-function wrapper that proxies to `rpc_get_author`.
#[cfg(not(feature = "mobile"))]
pub async fn get_author(_server_url: &str, id: i64) -> Result<Option<AuthorDetail>, DataError> {
    crate::rpc::rpc_get_author(id)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `scan_author_photo` — server-function wrapper that proxies to `rpc_scan_author_photo`.
#[cfg(not(feature = "mobile"))]
pub async fn scan_author_photo(
    _server_url: &str,
    id: i64,
) -> Result<AuthorPhotoScanResult, DataError> {
    crate::rpc::rpc_scan_author_photo(id)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `refetch_author_photos` — posts a worker task via server function.
#[cfg(not(feature = "mobile"))]
pub async fn refetch_author_photos(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_refetch_author_photos()
        .await
        .map_err(note_server_fn_err)
}

/// Admin "Delete author". Removes the author taxonomy row, drops every
/// `books_authors_link` for it, and adds the name to `ignored_authors`
/// so the next `indexer::reindex` does not silently resurrect the row.
/// Returns the number of books that were un-linked (used by the
/// confirmation modal's "this affects N books" copy). Web-only — mobile
/// parity is a deliberate follow-up.
#[cfg(not(feature = "mobile"))]
pub async fn delete_author(_server_url: &str, id: i64) -> Result<u64, DataError> {
    crate::rpc::rpc_delete_author(id)
        .await
        .map_err(note_server_fn_err)
}

/// Persist an author photo by URL. Web routes through the `#[post]`
/// server function in `rpc.rs`, which performs the server-side fetch +
/// validation and writes a `manual` row.
#[cfg(not(feature = "mobile"))]
pub async fn set_author_photo_url(
    _server_url: &str,
    id: i64,
    url: String,
) -> Result<(), DataError> {
    crate::rpc::rpc_set_author_photo_url(id, url)
        .await
        .map_err(note_server_fn_err)
}

/// Multipart upload of an author photo on the web client.
///
/// Server functions can't carry binary file uploads (they JSON-serialize
/// their arguments), so this bypasses RPC and POSTs the bytes directly to
/// the REST endpoint via `gloo-net`. The browser auto-attaches the
/// `omnibus_session` cookie on a same-origin request, so no manual auth
/// plumbing is needed. SSR doesn't call this — it only fires from a user
/// `onchange` handler after hydration.
#[cfg(feature = "web")]
pub async fn upload_author_photo(
    _server_url: &str,
    id: i64,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<(), DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let endpoint = format!("/api/authors/{id}/photo");
    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    // `Blob` ctor wants a `&Array` of `BufferSource | BlobPart` parts —
    // build a one-element Uint8Array, drop it into a JS Array, then hand
    // that to `Blob::new_with_u8_array_sequence_and_options`.
    let u8 = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let mut opts = web_sys::BlobPropertyBag::new();
    opts.type_(&mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))?;
    form.append_with_blob_and_filename("photo", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::put(&endpoint)
        // gloo-net's `body` takes anything `Into<JsValue>` — FormData
        // satisfies that via its JsCast impl. Don't set Content-Type:
        // the browser fills it in with the multipart boundary.
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if res.status() == 401 {
        super::web_auth_state::notify_unauthorized();
        return Err(DataError::Unauthorized);
    }
    if !res.ok() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(DataError::Http { status, body });
    }
    Ok(())
}

/// Fallback stub for the non-web, non-mobile build (cargo check on the
/// default workspace members compiles the frontend with no platform
/// feature so type-checking still passes). The author detail page only
/// invokes upload after a user `onchange`, which never fires under SSR.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_author_photo(
    _server_url: &str,
    _id: i64,
    _filename: String,
    _mime: String,
    _bytes: Vec<u8>,
) -> Result<(), DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

/// Web/SSR `list_authors` — server-function wrapper that proxies to `rpc_list_authors`.
#[cfg(not(feature = "mobile"))]
pub async fn list_authors(_server_url: &str) -> Result<Vec<AuthorSummary>, DataError> {
    crate::rpc::rpc_list_authors()
        .await
        .map_err(note_server_fn_err)
}
