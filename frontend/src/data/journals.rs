//! Public journal transport. Wraps create/list/update/delete/preview for mobile
//! (`/api/journals*` via reqwest) and web/SSR (RPC server functions in
//! `crate::rpc`). Mobile and web/SSR variants share each function's public
//! signature so callers stay platform-agnostic; the `#[cfg]` gates carry the
//! split.

use omnibus_shared::{CreateJournalEntry, JournalEntry, UpdateJournalEntry};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

// Mobile (REST).

/// POST `/api/journals` — create a journal entry.
#[cfg(feature = "mobile")]
pub async fn create_journal_entry(
    server_url: &str,
    input: CreateJournalEntry,
) -> Result<JournalEntry, DataError> {
    let url = format!("{server_url}/api/journals");
    let response = with_bearer(http_client().post(&url).json(&input))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<JournalEntry>().await?)
}

/// GET `/api/journals/book/{uuid}` — list all entries for a book (public).
#[cfg(feature = "mobile")]
pub async fn list_journal_entries(
    server_url: &str,
    book_uuid: &str,
) -> Result<Vec<JournalEntry>, DataError> {
    let url = format!("{server_url}/api/journals/book/{book_uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<JournalEntry>>().await?)
}

/// PATCH `/api/journals/{id}` — edit an entry owned by the current user.
#[cfg(feature = "mobile")]
pub async fn update_journal_entry(
    server_url: &str,
    id: i64,
    input: UpdateJournalEntry,
) -> Result<JournalEntry, DataError> {
    let url = format!("{server_url}/api/journals/{id}");
    let response = with_bearer(http_client().patch(&url).json(&input))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<JournalEntry>().await?)
}

/// DELETE `/api/journals/{id}` — delete an entry owned by the current user.
#[cfg(feature = "mobile")]
pub async fn delete_journal_entry(server_url: &str, id: i64) -> Result<(), DataError> {
    let url = format!("{server_url}/api/journals/{id}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// POST `/api/journals/preview` — render draft markdown to sanitized HTML.
#[cfg(feature = "mobile")]
pub async fn preview_journal_markdown(
    server_url: &str,
    body_md: String,
) -> Result<String, DataError> {
    let url = format!("{server_url}/api/journals/preview");
    let response = with_bearer(http_client().post(&url).json(&body_md))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<String>().await?)
}

/// POST `/api/journals/images` (mobile) — upload an image to embed in an
/// entry, returning its serving URL. Multipart, so it bypasses the
/// server-function transport like the author-photo/book uploads.
#[cfg(feature = "mobile")]
pub async fn upload_journal_image(
    server_url: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<String, DataError> {
    let endpoint = format!("{server_url}/api/journals/images");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime)?;
    let form = reqwest::multipart::Form::new().part("image", part);
    let response = with_bearer(http_client().post(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response
        .json::<omnibus_shared::JournalImageUpload>()
        .await?
        .url)
}

/// POST `/api/journals/images` (web) — upload an image to embed in an entry,
/// returning its serving URL. `gloo-net` + `FormData`, mirroring
/// `upload_author_photo`.
#[cfg(all(not(feature = "mobile"), feature = "web"))]
pub async fn upload_journal_image(
    _server_url: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<String, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    let u8 = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(&mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))?;
    form.append_with_blob_and_filename("image", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::post("/api/journals/images")
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if !res.ok() {
        if res.status() == 401 {
            super::web_auth_state::notify_unauthorized();
            return Err(DataError::Unauthorized);
        }
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(DataError::Http { status, body });
    }
    Ok(res
        .json::<omnibus_shared::JournalImageUpload>()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?
        .url)
}

/// Unavailable in this build — neither `web` nor `mobile` is enabled (SSR
/// never drives the composer's upload control).
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_journal_image(
    _server_url: &str,
    _filename: String,
    _mime: String,
    _bytes: Vec<u8>,
) -> Result<String, DataError> {
    Err(DataError::Other(
        "journal image upload not available in this build".into(),
    ))
}

// Web / SSR (RPC).

/// Web/SSR `create_journal_entry` — proxies to `rpc_create_journal_entry`.
#[cfg(not(feature = "mobile"))]
pub async fn create_journal_entry(
    _server_url: &str,
    input: CreateJournalEntry,
) -> Result<JournalEntry, DataError> {
    crate::rpc::rpc_create_journal_entry(input)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `list_journal_entries` — proxies to `rpc_list_journal_entries`.
#[cfg(not(feature = "mobile"))]
pub async fn list_journal_entries(
    _server_url: &str,
    book_uuid: &str,
) -> Result<Vec<JournalEntry>, DataError> {
    crate::rpc::rpc_list_journal_entries(book_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `update_journal_entry` — proxies to `rpc_update_journal_entry`.
#[cfg(not(feature = "mobile"))]
pub async fn update_journal_entry(
    _server_url: &str,
    id: i64,
    input: UpdateJournalEntry,
) -> Result<JournalEntry, DataError> {
    crate::rpc::rpc_update_journal_entry(id, input)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `delete_journal_entry` — proxies to `rpc_delete_journal_entry`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_journal_entry(_server_url: &str, id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_delete_journal_entry(id)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `preview_journal_markdown` — proxies to `rpc_preview_journal_markdown`.
#[cfg(not(feature = "mobile"))]
pub async fn preview_journal_markdown(
    _server_url: &str,
    body_md: String,
) -> Result<String, DataError> {
    crate::rpc::rpc_preview_journal_markdown(body_md)
        .await
        .map_err(note_server_fn_err)
}
