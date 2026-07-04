//! F4.3 Send-to-Kindle client wrappers: the per-user send + Kindle-email, and
//! the admin SMTP config (get/set/clear/test). Each has a web/SSR
//! server-function wrapper and a mobile `reqwest` variant with identical
//! signatures across the `#[cfg]` split.

use omnibus_shared::{SmtpConfigStatus, SmtpConfigUpdate};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

// ── Send to Kindle ───────────────────────────────────────────────

/// Web/SSR: email a book's EPUB to the user's Kindle address. Awaits the
/// worker, so an `Ok` means delivery succeeded and an `Err` carries the reason.
#[cfg(not(feature = "mobile"))]
pub async fn send_to_kindle(
    _server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<(), DataError> {
    crate::rpc::rpc_send_to_kindle(uuid.to_string(), file_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/kindle/send`.
#[cfg(feature = "mobile")]
pub async fn send_to_kindle(
    server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<(), DataError> {
    let url = format!("{server_url}/api/kindle/send");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "book_uuid": uuid, "file_id": file_id }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

// ── Per-user Kindle email ────────────────────────────────────────

/// Web/SSR: set (or clear, with `None`) the user's Kindle email.
#[cfg(not(feature = "mobile"))]
pub async fn set_kindle_email(_server_url: &str, email: Option<String>) -> Result<(), DataError> {
    crate::rpc::rpc_set_kindle_email(email)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/account/kindle-email`.
#[cfg(feature = "mobile")]
pub async fn set_kindle_email(server_url: &str, email: Option<String>) -> Result<(), DataError> {
    let url = format!("{server_url}/api/account/kindle-email");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

// ── SMTP config (admin settings) ─────────────────────────────────

/// Web/SSR: read the masked SMTP config status.
#[cfg(not(feature = "mobile"))]
pub async fn get_smtp_config(_server_url: &str) -> Result<SmtpConfigStatus, DataError> {
    crate::rpc::rpc_get_smtp_config()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: GET `/api/smtp`.
#[cfg(feature = "mobile")]
pub async fn get_smtp_config(server_url: &str) -> Result<SmtpConfigStatus, DataError> {
    let url = format!("{server_url}/api/smtp");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<SmtpConfigStatus>().await?)
}

/// Web/SSR: save the SMTP config, returning the new masked status.
#[cfg(not(feature = "mobile"))]
pub async fn set_smtp_config(
    _server_url: &str,
    update: SmtpConfigUpdate,
) -> Result<SmtpConfigStatus, DataError> {
    crate::rpc::rpc_set_smtp_config(update)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/smtp`.
#[cfg(feature = "mobile")]
pub async fn set_smtp_config(
    server_url: &str,
    update: SmtpConfigUpdate,
) -> Result<SmtpConfigStatus, DataError> {
    let url = format!("{server_url}/api/smtp");
    let response = with_bearer(http_client().post(&url))
        .json(&update)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<SmtpConfigStatus>().await?)
}

/// Web/SSR: clear the SMTP config, returning the (now unset) masked status.
#[cfg(not(feature = "mobile"))]
pub async fn clear_smtp_config(_server_url: &str) -> Result<SmtpConfigStatus, DataError> {
    crate::rpc::rpc_clear_smtp_config()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/smtp/clear`.
#[cfg(feature = "mobile")]
pub async fn clear_smtp_config(server_url: &str) -> Result<SmtpConfigStatus, DataError> {
    let url = format!("{server_url}/api/smtp/clear");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<SmtpConfigStatus>().await?)
}

/// Web/SSR: send a test email to the admin's own Kindle address.
#[cfg(not(feature = "mobile"))]
pub async fn send_smtp_test(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_send_smtp_test()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/smtp/test`.
#[cfg(feature = "mobile")]
pub async fn send_smtp_test(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/smtp/test");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}
