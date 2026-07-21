//! "Add your own books" upload helpers.
//!
//! Like the author-photo upload, these bypass the server-function transport
//! (which can't carry binary payloads) and post multipart bodies straight to
//! the REST endpoints: `gloo-net` + `FormData` on web, `reqwest::multipart` on
//! mobile. The two-step shape — `inspect_ebook` then `upload_ebook` — lets the
//! UI show an editable confirm step before the file is filed.

use omnibus_shared::{AudiobookInspection, UploadCommitResult, UploadInspection};

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

/// The user's confirmed metadata for an audiobook commit. `title`/`author`
/// drive the on-disk folder; audiobooks carry no series fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudiobookUploadMeta {
    pub title: String,
    pub author: String,
}

/// Multipart content-type for an audiobook part, keyed off its extension. The
/// server re-derives the format from magic bytes, so this is advisory only.
#[cfg(any(feature = "web", feature = "mobile"))]
fn audio_mime(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".mp3") {
        "audio/mpeg"
    } else if lower.ends_with(".m4a") || lower.ends_with(".m4b") {
        "audio/mp4"
    } else {
        "application/octet-stream"
    }
}

// Web (gloo-net + FormData).

/// Build a one-shot `Blob` from raw bytes with the given MIME type.
#[cfg(feature = "web")]
fn ebook_blob(bytes: &[u8]) -> Result<web_sys::Blob, DataError> {
    let u8 = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/epub+zip");
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

/// Upload the file for server-side inspection ahead of the confirm step.
#[cfg(feature = "web")]
pub async fn inspect_ebook(
    _server_url: &str,
    filename: String,
    bytes: &[u8],
) -> Result<UploadInspection, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    let blob = ebook_blob(bytes)?;
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

/// Commit a previously-inspected ebook to the library with the confirmed metadata.
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

/// Append one or more audiobook parts to a `FormData` under the `file` field,
/// each with a typed `Blob`.
#[cfg(feature = "web")]
fn append_audio_parts(
    form: &web_sys::FormData,
    files: &[(String, Vec<u8>)],
) -> Result<(), DataError> {
    for (name, bytes) in files {
        let u8 = js_sys::Uint8Array::from(bytes.as_slice());
        let parts = js_sys::Array::new();
        parts.push(&u8);
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type(audio_mime(name));
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
            .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))?;
        form.append_with_blob_and_filename("file", &blob, name)
            .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;
    }
    Ok(())
}

/// Upload the audiobook part(s) for server-side inspection ahead of the confirm
/// step. `files` is one `.m4a`/`.m4b` container or the ordered `.mp3` parts.
#[cfg(feature = "web")]
pub async fn inspect_audiobook(
    _server_url: &str,
    files: &[(String, Vec<u8>)],
) -> Result<AudiobookInspection, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    append_audio_parts(&form, files)?;

    let res = Request::post("/api/uploads/audiobooks/inspect")
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if !res.ok() {
        return Err(web_error(res).await);
    }
    res.json::<AudiobookInspection>()
        .await
        .map_err(|e| DataError::Other(e.to_string()))
}

/// Commit a previously-inspected audiobook to the library with the confirmed
/// metadata.
#[cfg(feature = "web")]
pub async fn upload_audiobook(
    _server_url: &str,
    files: Vec<(String, Vec<u8>)>,
    meta: AudiobookUploadMeta,
) -> Result<UploadCommitResult, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    form.append_with_str("title", &meta.title)
        .map_err(|e| DataError::Other(format!("FormData::append title: {e:?}")))?;
    form.append_with_str("author", &meta.author)
        .map_err(|e| DataError::Other(format!("FormData::append author: {e:?}")))?;
    append_audio_parts(&form, &files)?;

    let res = Request::post("/api/uploads/audiobooks")
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

// Mobile (reqwest multipart).

/// Upload the file for server-side inspection ahead of the confirm step.
#[cfg(feature = "mobile")]
pub async fn inspect_ebook(
    server_url: &str,
    filename: String,
    bytes: &[u8],
) -> Result<UploadInspection, DataError> {
    let endpoint = format!("{server_url}/api/uploads/ebooks/inspect");
    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
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

/// Commit a previously-inspected ebook to the library with the confirmed metadata.
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

/// Upload the audiobook part(s) for server-side inspection ahead of the confirm
/// step.
#[cfg(feature = "mobile")]
pub async fn inspect_audiobook(
    server_url: &str,
    files: &[(String, Vec<u8>)],
) -> Result<AudiobookInspection, DataError> {
    let endpoint = format!("{server_url}/api/uploads/audiobooks/inspect");
    let mut form = reqwest::multipart::Form::new();
    for (name, bytes) in files {
        let part = reqwest::multipart::Part::bytes(bytes.clone())
            .file_name(name.clone())
            .mime_str(audio_mime(name))?;
        form = form.part("file", part);
    }
    let response = with_bearer(http_client().post(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<AudiobookInspection>().await?)
}

/// Commit a previously-inspected audiobook to the library with the confirmed
/// metadata.
#[cfg(feature = "mobile")]
pub async fn upload_audiobook(
    server_url: &str,
    files: Vec<(String, Vec<u8>)>,
    meta: AudiobookUploadMeta,
) -> Result<UploadCommitResult, DataError> {
    let endpoint = format!("{server_url}/api/uploads/audiobooks");
    let mut form = reqwest::multipart::Form::new()
        .text("title", meta.title)
        .text("author", meta.author);
    for (name, bytes) in files {
        let mime = audio_mime(&name);
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name)
            .mime_str(mime)?;
        form = form.part("file", part);
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

// Fallback stub (SSR / no platform feature).

/// Unavailable in this build — neither `web` nor `mobile` is enabled.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn inspect_ebook(
    _server_url: &str,
    _filename: String,
    _bytes: &[u8],
) -> Result<UploadInspection, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

/// Unavailable in this build — neither `web` nor `mobile` is enabled.
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

/// Unavailable in this build — neither `web` nor `mobile` is enabled.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn inspect_audiobook(
    _server_url: &str,
    _files: &[(String, Vec<u8>)],
) -> Result<AudiobookInspection, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

/// Unavailable in this build — neither `web` nor `mobile` is enabled.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_audiobook(
    _server_url: &str,
    _files: Vec<(String, Vec<u8>)>,
    _meta: AudiobookUploadMeta,
) -> Result<UploadCommitResult, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}
