//! Metadata-override, cover, merge, and deletion mutations — the write
//! side of the books domain. Merge and deletion are web-admin-only; their
//! mobile variants are typed stubs so callers compile across the split.

use omnibus_shared::{
    BookDeletionManifest, DeleteBookFilesResult, EbookMetadata, MergeBooksResult, MetadataOverrides,
};

#[cfg(not(feature = "mobile"))]
use crate::data::note_server_fn_err;
use crate::data::DataError;
#[cfg(feature = "mobile")]
use crate::data::{drain_error, http_client, note_status, with_bearer};

/// POST `/api/ebooks/{uuid}/overrides` — persist user metadata overrides.
#[cfg(feature = "mobile")]
pub async fn save_overrides(
    server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().post(&url))
        .json(overrides)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// DELETE `/api/ebooks/{uuid}/overrides` — revert to original metadata.
#[cfg(feature = "mobile")]
pub async fn delete_overrides(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// Multipart upload of a replacement cover for a book (mobile). Mirrors
/// `upload_author_photo`'s mobile REST path against the analogous
/// `/api/ebooks/:uuid/cover` endpoint.
#[cfg(feature = "mobile")]
pub async fn upload_ebook_cover(
    server_url: &str,
    uuid: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let endpoint = format!("{server_url}/api/ebooks/{uuid}/cover");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime)?;
    let form = reqwest::multipart::Form::new().part("cover", part);
    let response = with_bearer(http_client().post(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// DELETE `/api/ebooks/{uuid}/cover` — revert an overridden cover to the
/// scanned original, preserving any other field overrides.
#[cfg(feature = "mobile")]
pub async fn delete_ebook_cover(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/ebooks/{uuid}/cover");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// Admin: merge the book `source_uuid` into `target_uuid`. Web-only —
/// mobile has no admin surface.
#[cfg(not(feature = "mobile"))]
pub async fn merge_books(
    _server_url: &str,
    source_uuid: &str,
    target_uuid: &str,
) -> Result<MergeBooksResult, DataError> {
    crate::rpc::rpc_merge_books(source_uuid.to_string(), target_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `merge_books` — merge is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn merge_books(
    _server_url: &str,
    _source_uuid: &str,
    _target_uuid: &str,
) -> Result<MergeBooksResult, DataError> {
    Err(DataError::Other("merge is web-only".into()))
}

/// Admin: candidate search for the merge dialog — FTS across both
/// configured libraries (unlike `search_ebooks`, which is ebook-only).
#[cfg(not(feature = "mobile"))]
pub async fn merge_candidates(_server_url: &str, q: &str) -> Result<Vec<EbookMetadata>, DataError> {
    crate::rpc::rpc_merge_candidates(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `merge_candidates` — merge is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn merge_candidates(
    _server_url: &str,
    _q: &str,
) -> Result<Vec<EbookMetadata>, DataError> {
    Err(DataError::Other("merge is web-only".into()))
}

/// Admin: undo a merge by its `merge_log` id. Returns the restored uuid.
#[cfg(not(feature = "mobile"))]
pub async fn undo_merge(_server_url: &str, merge_log_id: i64) -> Result<String, DataError> {
    crate::rpc::rpc_undo_merge(merge_log_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `undo_merge` — merge is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn undo_merge(_server_url: &str, _merge_log_id: i64) -> Result<String, DataError> {
    Err(DataError::Other("merge is web-only".into()))
}

/// Admin: the book's deletable items plus the user data a total delete would
/// take with them. Web-only — mobile has no admin surface.
#[cfg(not(feature = "mobile"))]
pub async fn book_deletion_manifest(
    _server_url: &str,
    uuid: &str,
) -> Result<BookDeletionManifest, DataError> {
    crate::rpc::rpc_book_deletion_manifest(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `book_deletion_manifest` — deletion is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn book_deletion_manifest(
    _server_url: &str,
    _uuid: &str,
) -> Result<BookDeletionManifest, DataError> {
    Err(DataError::Other("deletion is web-only".into()))
}

/// Admin: delete the given files and physical copies; when no item is left,
/// the book goes too.
#[cfg(not(feature = "mobile"))]
pub async fn delete_book_files(
    _server_url: &str,
    uuid: &str,
    file_ids: Vec<i64>,
    copy_ids: Vec<i64>,
) -> Result<DeleteBookFilesResult, DataError> {
    crate::rpc::rpc_delete_book_files(uuid.to_string(), file_ids, copy_ids)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `delete_book_files` — deletion is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn delete_book_files(
    _server_url: &str,
    _uuid: &str,
    _file_ids: Vec<i64>,
    _copy_ids: Vec<i64>,
) -> Result<DeleteBookFilesResult, DataError> {
    Err(DataError::Other("deletion is web-only".into()))
}

/// Web/SSR `save_overrides` — server-function wrapper that proxies to `rpc_save_overrides`.
#[cfg(not(feature = "mobile"))]
pub async fn save_overrides(
    _server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_save_overrides(uuid.to_string(), overrides.clone())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `delete_overrides` — server-function wrapper that proxies to `rpc_delete_overrides`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_overrides(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_delete_overrides(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Multipart upload of a replacement cover on the web client.
///
/// Server functions can't carry binary file uploads (they JSON-serialize
/// their arguments), so this bypasses RPC and POSTs directly to the REST
/// endpoint via `gloo-net`, mirroring `upload_author_photo`'s web path.
#[cfg(feature = "web")]
pub async fn upload_ebook_cover(
    _server_url: &str,
    uuid: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<Option<EbookMetadata>, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let endpoint = format!("/api/ebooks/{uuid}/cover");
    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    // See `upload_author_photo`'s web impl for why this goes through a
    // one-element `Uint8Array` → `Array` → `Blob` rather than a direct
    // byte-slice constructor.
    let u8 = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(&mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))?;
    form.append_with_blob_and_filename("cover", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::post(&endpoint)
        // Don't set Content-Type: the browser fills it in with the
        // multipart boundary.
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if res.status() == 401 {
        crate::data::web_auth_state::notify_unauthorized();
        return Err(DataError::Unauthorized);
    }
    if !res.ok() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(DataError::Http { status, body });
    }
    let book = res
        .json::<EbookMetadata>()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    Ok(Some(book))
}

/// Fallback stub for the non-web, non-mobile build (cargo check on the
/// default workspace members compiles the frontend with no platform
/// feature so type-checking still passes). The metadata edit sidebar only
/// invokes upload after a user `onchange`, which never fires under SSR.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_ebook_cover(
    _server_url: &str,
    _uuid: &str,
    _filename: String,
    _mime: String,
    _bytes: Vec<u8>,
) -> Result<Option<EbookMetadata>, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

/// Web/SSR `delete_ebook_cover` — server-function wrapper that proxies to
/// `rpc_delete_ebook_cover`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_ebook_cover(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_delete_ebook_cover(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
