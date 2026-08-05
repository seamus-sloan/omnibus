//! Per-user profile writes: the display name and the avatar image.
//!
//! The name goes through the RPC server function like the other Account
//! writes; the avatar can't (a server function JSON-serializes its arguments),
//! so it POSTs multipart straight to `/api/account/avatar` — the same split the
//! author-photo upload makes.
//!
//! Neither is ever queued offline: a profile is account configuration, so a
//! failure must surface immediately rather than replay later (rule 08).

use super::DataError;

// ── Display name ─────────────────────────────────────────────────────

/// Web/SSR: set (or clear, with `None`) the caller's display name.
#[cfg(not(feature = "mobile"))]
pub async fn set_display_name(
    _server_url: &str,
    display_name: Option<String>,
) -> Result<(), DataError> {
    crate::rpc::rpc_set_display_name(display_name)
        .await
        .map_err(super::note_server_fn_err)
}

/// Mobile: POST `/api/account/profile`.
#[cfg(feature = "mobile")]
pub async fn set_display_name(
    server_url: &str,
    display_name: Option<String>,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/account/profile");
    let response = super::with_bearer(super::http_client().post(&url))
        .json(&serde_json::json!({ "display_name": display_name }))
        .send()
        .await?;
    let status = super::note_status(response.status());
    if !status.is_success() {
        return Err(super::drain_error(response, status).await);
    }
    Ok(())
}

/// Fallback for the non-web, non-mobile build (plain `cargo check` on the
/// default workspace members compiles the frontend with no platform feature).
#[cfg(not(any(feature = "web", feature = "server", feature = "mobile")))]
pub async fn set_display_name(
    _server_url: &str,
    _display_name: Option<String>,
) -> Result<(), DataError> {
    Ok(())
}

// ── Avatar ───────────────────────────────────────────────────────────

/// Web: upload the caller's avatar as multipart. Mirrors
/// `upload_author_photo` — a server function can't carry bytes, and the
/// browser attaches the session cookie itself on a same-origin request.
#[cfg(feature = "web")]
pub async fn upload_avatar(
    _server_url: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<(), DataError> {
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
    form.append_with_blob_and_filename("avatar", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::post("/api/account/avatar")
        // Don't set Content-Type — the browser fills in the multipart
        // boundary.
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

/// Mobile: the same upload over `reqwest`'s multipart.
#[cfg(feature = "mobile")]
pub async fn upload_avatar(
    server_url: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<(), DataError> {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime)
        .map_err(|e| DataError::Other(e.to_string()))?;
    let form = reqwest::multipart::Form::new().part("avatar", part);
    let url = format!("{server_url}/api/account/avatar");
    let response = super::with_bearer(super::http_client().post(&url))
        .multipart(form)
        .send()
        .await?;
    let status = super::note_status(response.status());
    if !status.is_success() {
        return Err(super::drain_error(response, status).await);
    }
    Ok(())
}

/// Fallback for the non-web, non-mobile build.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_avatar(
    _server_url: &str,
    _filename: String,
    _mime: String,
    _bytes: Vec<u8>,
) -> Result<(), DataError> {
    Ok(())
}

/// Web: remove the caller's avatar, reverting them to a monogram.
#[cfg(feature = "web")]
pub async fn delete_avatar(_server_url: &str) -> Result<(), DataError> {
    use gloo_net::http::Request;

    let res = Request::delete("/api/account/avatar")
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

/// Mobile: same delete over `reqwest`.
#[cfg(feature = "mobile")]
pub async fn delete_avatar(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/account/avatar");
    let response = super::with_bearer(super::http_client().delete(&url))
        .send()
        .await?;
    let status = super::note_status(response.status());
    if !status.is_success() {
        return Err(super::drain_error(response, status).await);
    }
    Ok(())
}

/// Fallback for the non-web, non-mobile build.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn delete_avatar(_server_url: &str) -> Result<(), DataError> {
    Ok(())
}
