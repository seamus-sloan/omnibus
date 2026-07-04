//! F4.3 Send-to-Kindle server function. Posts a `SendToKindle` job to the
//! shared worker and awaits its completion so the caller gets a synchronous
//! Sent / failed result (SMTP is slow but this is a discrete user action).

use dioxus::fullstack::post;
use dioxus::prelude::*;

#[cfg(feature = "server")]
use omnibus_db::{self as db, worker::TaskOutcome};

#[cfg(feature = "server")]
use super::{AuthUser, PoolExt, WorkerExt};

/// Email the EPUB for `book_uuid` (optionally a specific `file_id` for
/// multi-EPUB books) to the authenticated user's configured Kindle address.
/// Errors when the user has no Kindle email, when SMTP is unconfigured, or when
/// delivery fails — the message is surfaced to the button UI.
#[post("/api/rpc/kindle/send", pool: PoolExt, worker: WorkerExt, user: AuthUser)]
pub async fn rpc_send_to_kindle(book_uuid: String, file_id: Option<i64>) -> Result<()> {
    let recipient = db::auth::get_kindle_email(&pool.0, user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let Some(recipient) = recipient else {
        return Err(ServerFnError::new(
            "add your Kindle email on your account page before sending",
        )
        .into());
    };
    if db::effective_smtp_config(&pool.0).await?.is_none() {
        return Err(ServerFnError::new("email delivery is not configured on this server").into());
    }
    let Some(book_id) = db::resolve_book_id_by_uuid(&pool.0, &book_uuid).await? else {
        return Err(ServerFnError::new("book not found").into());
    };

    let id = worker.0.post(omnibus_db::worker::Task::SendToKindle {
        book_id,
        book_file_id: file_id,
        recipient_email: recipient,
    });
    match worker.0.await_completion(id).await {
        TaskOutcome::Ok => Ok(()),
        TaskOutcome::Err(msg) => Err(ServerFnError::new(msg).into()),
    }
}
