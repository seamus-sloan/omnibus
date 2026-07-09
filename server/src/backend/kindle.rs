//! F4.3 Send-to-Kindle REST handlers (mobile-facing). Mirrors the web server
//! functions in `frontend/src/rpc/{kindle,account}.rs` and the SMTP admin RPCs
//! in `frontend/src/rpc/settings.rs`: per-user send + Kindle-email, and the
//! admin-only SMTP config (get / set / clear / test).

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, worker::Task};
use omnibus_shared::{KindleSendStatus, ProgressState, SmtpConfigUpdate};
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::{AdminUser, AuthUser};

/// Body for `POST /api/kindle/send`.
#[derive(Debug, Deserialize)]
pub(super) struct SendBody {
    book_uuid: String,
    #[serde(default)]
    file_id: Option<i64>,
}

/// Enqueue a send of the EPUB to the authenticated user's Kindle address and
/// return the worker `task_id` to poll (`{"task_id": N}`). Fast pre-checks (no
/// Kindle email, SMTP unconfigured, unknown book) still fail synchronously; the
/// SMTP delivery runs on the worker, polled via `GET /api/kindle/send/status`.
pub(super) async fn post_send(
    user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<SendBody>,
) -> Response {
    let recipient = match db::auth::get_kindle_email(&state.pool, user.id).await {
        Ok(Some(email)) => email,
        Ok(None) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "add your Kindle email on your account page before sending",
            )
                .into_response();
        }
        Err(e) => return internal("read kindle email", e),
    };
    match db::effective_smtp_config(&state.pool).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                "email delivery is not configured on this server",
            )
                .into_response();
        }
        Err(e) => return internal("read smtp config", e),
    }
    let book_id = match db::resolve_book_id_by_uuid(&state.pool, &body.book_uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve book", e),
    };

    let task_id = state.worker.post(Task::SendToKindle {
        book_id,
        book_file_id: body.file_id,
        recipient_email: recipient,
    });
    // Scope the pollable status to this user so the guessable task-id space
    // can't be probed for other users' send outcomes.
    state.worker.set_task_owner(task_id, user.id);
    Json(serde_json::json!({ "task_id": task_id })).into_response()
}

/// Query for `GET /api/kindle/send/status`.
#[derive(Debug, Deserialize)]
pub(super) struct SendStatusQuery {
    task_id: u64,
}

/// Poll a send enqueued by [`post_send`]. Returns the [`KindleSendStatus`] for
/// `task_id`, or 404 once it's unknown (never posted, evicted after the
/// worker's terminal-retention window, or not owned by the caller). Scoped to
/// the requesting user so the guessable task-id space can't be probed for
/// other users' send outcomes.
pub(super) async fn get_send_status(
    user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<SendStatusQuery>,
) -> Response {
    match state.worker.owned_task_state(q.task_id, user.id) {
        Some(ProgressState::Running { .. }) => Json(KindleSendStatus::Pending).into_response(),
        Some(ProgressState::Done { .. }) => Json(KindleSendStatus::Sent).into_response(),
        Some(ProgressState::Failed { message }) => {
            Json(KindleSendStatus::Failed { message }).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Body for `POST /api/account/kindle-email`. A `null`/absent/blank email
/// clears it.
#[derive(Debug, Deserialize)]
pub(super) struct SetKindleEmail {
    #[serde(default)]
    email: Option<String>,
}

/// Set or clear the authenticated user's Kindle email. 422 on a malformed
/// address.
pub(super) async fn post_kindle_email(
    user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<SetKindleEmail>,
) -> Response {
    match db::auth::set_kindle_email(&state.pool, user.id, body.email.as_deref()).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(db::auth::AuthError::Validation(msg)) => {
            (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response()
        }
        Err(e) => internal("set kindle email", e),
    }
}

/// Admin-only: masked SMTP config status.
pub(super) async fn get_smtp(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::smtp_status(&state.pool).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal("read smtp config", e),
    }
}

/// Admin-only: save the SMTP config; returns the new masked status. 422 on a
/// validation failure.
pub(super) async fn post_smtp(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(update): Json<SmtpConfigUpdate>,
) -> Response {
    match db::set_smtp_config(&state.pool, &update).await {
        Ok(()) => {}
        Err(db::SettingsError::Validation(msg)) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
        }
        Err(e) => return internal("save smtp config", e),
    }
    match db::smtp_status(&state.pool).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal("read smtp config", e),
    }
}

/// Admin-only: clear the SMTP config.
pub(super) async fn post_smtp_clear(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::clear_smtp_config(&state.pool).await {
        Ok(()) => match db::smtp_status(&state.pool).await {
            Ok(s) => Json(s).into_response(),
            Err(e) => internal("read smtp config", e),
        },
        Err(e) => internal("clear smtp config", e),
    }
}

/// Admin-only: send a test email to the admin's own Kindle address.
pub(super) async fn post_smtp_test(admin: AdminUser, State(state): State<AppState>) -> Response {
    let recipient = match db::auth::get_kindle_email(&state.pool, admin.0.id).await {
        Ok(Some(email)) => email,
        Ok(None) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "set your Kindle email on your account page first, then send a test",
            )
                .into_response();
        }
        Err(e) => return internal("read kindle email", e),
    };
    match db::kindle::send_test(&state.pool, &recipient).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(db::kindle::KindleError::NotConfigured) => (
            StatusCode::CONFLICT,
            "email delivery is not configured on this server",
        )
            .into_response(),
        // `NoEpub`/`Timeout` carry a safe, specific message; the remaining
        // variants (`Io`/`Address`/`Build`/`Smtp`/`Settings`/`Books`) wrap
        // opaque transport/DB internals and must not reach the client.
        Err(e @ (db::kindle::KindleError::NoEpub | db::kindle::KindleError::Timeout)) => {
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
        Err(e) => internal("send test email", e),
    }
}

#[cfg(test)]
mod tests;
