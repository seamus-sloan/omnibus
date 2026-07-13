//! Send-to-Kindle client wrappers: the per-user send + Kindle-email, and
//! the admin SMTP config (get/set/clear/test). Each has a web/SSR
//! server-function wrapper and a mobile `reqwest` variant with identical
//! signatures across the `#[cfg]` split.

use omnibus_shared::{KindleSendStatus, SmtpConfigStatus, SmtpConfigUpdate};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

// ── Send to Kindle ───────────────────────────────────────────────
//
// Send is enqueue-and-poll: `enqueue_send_to_kindle` posts the job and returns
// the worker `task_id` (fast, so it stays under the server's 30s request-timeout
// guard), then the caller polls `kindle_send_status` until it flips off
// `Pending`. Blocking the request on the SMTP delivery instead risked a hung
// relay tripping that guard and stalling the UI on "Sending…".

/// Web/SSR: enqueue a send of the book's EPUB and return the worker `task_id`
/// to poll. An `Err` here is a fast pre-check failure (no Kindle email, SMTP
/// unconfigured, unknown book), surfaced immediately.
#[cfg(not(feature = "mobile"))]
pub async fn enqueue_send_to_kindle(
    _server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<u64, DataError> {
    crate::rpc::rpc_send_to_kindle(uuid.to_string(), file_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/kindle/send`, returning the worker `task_id`.
#[cfg(feature = "mobile")]
pub async fn enqueue_send_to_kindle(
    server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<u64, DataError> {
    let url = format!("{server_url}/api/kindle/send");
    let response = with_bearer(http_client().post(&url))
        .json(&serde_json::json!({ "book_uuid": uuid, "file_id": file_id }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    #[derive(serde::Deserialize)]
    struct EnqueueResp {
        task_id: u64,
    }
    Ok(response.json::<EnqueueResp>().await?.task_id)
}

/// Web/SSR: poll a send's status. `None` means the `task_id` is unknown (never
/// posted, or evicted after the worker's terminal-retention window).
#[cfg(not(feature = "mobile"))]
pub async fn kindle_send_status(
    _server_url: &str,
    task_id: u64,
) -> Result<Option<KindleSendStatus>, DataError> {
    crate::rpc::rpc_kindle_send_status(task_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: GET `/api/kindle/send/status`. A 404 maps to `None` (unknown id).
#[cfg(feature = "mobile")]
pub async fn kindle_send_status(
    server_url: &str,
    task_id: u64,
) -> Result<Option<KindleSendStatus>, DataError> {
    let url = format!("{server_url}/api/kindle/send/status?task_id={task_id}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<KindleSendStatus>().await?))
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
