//! "Add your own books" upload helpers.
//!
//! Like the author-photo upload, these bypass the server-function transport
//! (which can't carry binary payloads) and post multipart bodies straight to
//! the REST endpoints: `gloo-net` + `FormData` on web, `reqwest::multipart` on
//! mobile. The two-step shape — `inspect_ebook` then `upload_ebook` — lets the
//! UI show an editable confirm step before the file is filed.

use omnibus_shared::{UploadCommitResult, UploadInspection};

use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// The user's confirmed metadata for the commit step. `title`/`author` are
/// required (they drive the on-disk folder); `series` fields are optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EbookUploadMeta {
    pub title: String,
    pub author: String,
    pub series: String,
    pub series_index: String,
}

// --- Web (gloo-net + FormData) ---------------------------------------------

/// Build a one-shot `Blob` from raw bytes with the given MIME type.
#[cfg(feature = "web")]
fn ebook_blob(bytes: &[u8]) -> Result<web_sys::Blob, DataError> {
    let u8 = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let mut opts = web_sys::BlobPropertyBag::new();
    opts.type_("application/epub+zip");
    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))
}

/// Map a non-2xx web response to a `DataError`, surfacing 401 to the auth
/// state so the app can redirect to login. Consumes `res`.
#[cfg(feature = "web")]
async fn web_error(res: gloo_net::http::Response) -> DataError {
    if res.status() == 401 {
        super::web_auth_state::notify_unauthorized();
        return DataError::Unauthorized;
    }
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    DataError::Http { status, body }
}

#[cfg(feature = "web")]
pub async fn inspect_ebook(
    _server_url: &str,
    filename: String,
    bytes: Vec<u8>,
) -> Result<UploadInspection, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    let blob = ebook_blob(&bytes)?;
    form.append_with_blob_and_filename("file", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::post("/api/uploads/ebooks/inspect")
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if !res.ok() {
        return Err(web_error(res).await);
    }
    res.json::<UploadInspection>()
        .await
        .map_err(|e| DataError::Other(e.to_string()))
}

#[cfg(feature = "web")]
pub async fn upload_ebook(
    _server_url: &str,
    filename: String,
    bytes: Vec<u8>,
    meta: EbookUploadMeta,
) -> Result<UploadCommitResult, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    let append_str = |name: &str, value: &str| -> Result<(), DataError> {
        form.append_with_str(name, value)
            .map_err(|e| DataError::Other(format!("FormData::append {name}: {e:?}")))
    };
    append_str("title", &meta.title)?;
    append_str("author", &meta.author)?;
    if !meta.series.trim().is_empty() {
        append_str("series", &meta.series)?;
    }
    if !meta.series_index.trim().is_empty() {
        append_str("series_index", &meta.series_index)?;
    }
    let blob = ebook_blob(&bytes)?;
    form.append_with_blob_and_filename("file", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::post("/api/uploads/ebooks")
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if !res.ok() {
        return Err(web_error(res).await);
    }
    res.json::<UploadCommitResult>()
        .await
        .map_err(|e| DataError::Other(e.to_string()))
}

// --- Mobile (reqwest multipart) --------------------------------------------

#[cfg(feature = "mobile")]
pub async fn inspect_ebook(
    server_url: &str,
    filename: String,
    bytes: Vec<u8>,
) -> Result<UploadInspection, DataError> {
    let endpoint = format!("{server_url}/api/uploads/ebooks/inspect");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str("application/epub+zip")?;
    let form = reqwest::multipart::Form::new().part("file", part);
    let response = with_bearer(http_client().post(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<UploadInspection>().await?)
}

#[cfg(feature = "mobile")]
pub async fn upload_ebook(
    server_url: &str,
    filename: String,
    bytes: Vec<u8>,
    meta: EbookUploadMeta,
) -> Result<UploadCommitResult, DataError> {
    let endpoint = format!("{server_url}/api/uploads/ebooks");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str("application/epub+zip")?;
    let mut form = reqwest::multipart::Form::new()
        .text("title", meta.title)
        .text("author", meta.author)
        .part("file", part);
    if !meta.series.trim().is_empty() {
        form = form.text("series", meta.series);
    }
    if !meta.series_index.trim().is_empty() {
        form = form.text("series_index", meta.series_index);
    }
    let response = with_bearer(http_client().post(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<UploadCommitResult>().await?)
}

// --- Fallback stub (SSR / no platform feature) -----------------------------

#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn inspect_ebook(
    _server_url: &str,
    _filename: String,
    _bytes: Vec<u8>,
) -> Result<UploadInspection, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_ebook(
    _server_url: &str,
    _filename: String,
    _bytes: Vec<u8>,
    _meta: EbookUploadMeta,
) -> Result<UploadCommitResult, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}
