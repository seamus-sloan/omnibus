//! F4.3 Send-to-Kindle server functions. `rpc_send_to_kindle` enqueues a
//! `SendToKindle` job on the shared worker and returns its `task_id`
//! immediately; the client then polls `rpc_kindle_send_status` for the
//! terminal result. Returning right away keeps the request well under the
//! server's 30s timeout guard, so a slow/hung SMTP relay never leaves the
//! request open (and thus never produces the retryable 408 that stalled the
//! button on "Sending…").

use dioxus::fullstack::post;
use dioxus::prelude::*;

use omnibus_shared::KindleSendStatus;
#[cfg(feature = "server")]
use omnibus_shared::ProgressState;

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{AuthUser, PoolExt, WorkerExt};

/// Enqueue a send of the EPUB for `book_uuid` (optionally a specific `file_id`
/// for multi-EPUB books) to the authenticated user's Kindle address, returning
/// the worker `task_id` to poll. Errors synchronously when the user has no
/// Kindle email, when SMTP is unconfigured, or when the book can't be resolved
/// — those are fast checks the button surfaces immediately. The actual SMTP
/// delivery runs on the worker; poll [`rpc_kindle_send_status`] for its result.
#[post("/api/rpc/kindle/send", pool: PoolExt, worker: WorkerExt, user: AuthUser)]
pub async fn rpc_send_to_kindle(book_uuid: String, file_id: Option<i64>) -> Result<u64> {
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
    // Scope the pollable status to this user so the guessable task-id space
    // can't be probed for other users' send outcomes.
    worker.0.set_task_owner(id, user.id);
    Ok(id)
}

/// Poll the status of a send enqueued by [`rpc_send_to_kindle`]. Returns
/// `None` once `task_id` is unknown — never posted, evicted after the worker's
/// terminal-retention window (~10s past completion), or not owned by the
/// caller. Scoped to the requesting user so the guessable task-id space can't
/// be probed for other users' send outcomes. The client polls this until it
/// observes a non-`Pending` status.
#[post("/api/rpc/kindle/send/status", _pool: PoolExt, worker: WorkerExt, user: AuthUser)]
pub async fn rpc_kindle_send_status(task_id: u64) -> Result<Option<KindleSendStatus>> {
    Ok(worker
        .0
        .owned_task_state(task_id, user.id)
        .map(|state| match state {
            ProgressState::Running { .. } => KindleSendStatus::Pending,
            ProgressState::Done { .. } => KindleSendStatus::Sent,
            ProgressState::Failed { message } => KindleSendStatus::Failed { message },
        }))
}
