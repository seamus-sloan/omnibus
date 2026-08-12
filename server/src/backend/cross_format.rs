//! Cross-format resume read endpoint: serves the mapped "resume in the
//! other format" candidate for a linked dual-format book. Read-only — the
//! write side (confirm/unlink) ships with the alignment modal.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use omnibus_shared::ProgressFormat;
use serde::Deserialize;

use omnibus_db as db;

use super::{internal, AppState};
use crate::auth::AuthUser;

#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize)]
pub(super) struct ResumeQuery {
    /// The format the client wants to resume in; the other format's
    /// position is the mapping source.
    target: ProgressFormat,
}

/// `GET /api/books/{uuid}/cross-format-resume?target=` — the mapped resume
/// candidate plus the state that explains a missing one (`not_linked`,
/// `link_stale`, `nothing_newer`). 404 only for a book the server has
/// never indexed.
pub(super) async fn get_cross_format_resume(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(q): Query<ResumeQuery>,
) -> Response {
    match db::cross_format::resume_candidate(&state.pool, user.id, &uuid, q.target).await {
        Ok(resume) => Json(resume).into_response(),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            (axum::http::StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => internal("cross_format_resume", e),
    }
}

/// `GET /api/books/{uuid}/alignment` — the alignment payload the native
/// clients' sheet renders (mirrors `rpc_get_alignment`).
pub(super) async fn get_alignment(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::cross_format::alignment_view(&state.pool, user.id, &uuid).await {
        Ok(view) => Json(view).into_response(),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            (axum::http::StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => internal("get_alignment", e),
    }
}

/// `POST /api/books/{uuid}/cross-format-link` — confirm (or re-confirm)
/// the link; the path uuid is authoritative over the body's. Mirrors
/// `rpc_confirm_cross_format_link`, including the `can_edit` gate on the
/// library-wide re-order half.
pub(super) async fn post_cross_format_link(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(mut update): Json<omnibus_shared::ConfirmCrossFormatLink>,
) -> Response {
    update.book_uuid = uuid;
    if let Err(msg) = update.validate() {
        return (axum::http::StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
    }
    if let Some(order) = &update.audio_order {
        if !user.can_edit {
            return (
                axum::http::StatusCode::FORBIDDEN,
                "reordering audio files requires edit permission",
            )
                .into_response();
        }
        match db::cross_format::set_audio_order(&state.pool, &update.book_uuid, order).await {
            Ok(()) => {}
            Err(db::cross_format::CrossFormatError::BookNotFound) => {
                return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response();
            }
            Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
                return (axum::http::StatusCode::CONFLICT, e.to_string()).into_response();
            }
            Err(db::cross_format::CrossFormatError::Sqlx(e)) => {
                return internal("set_audio_order", e);
            }
        }
    }
    match db::cross_format::upsert_link(
        &state.pool,
        user.id,
        &update.book_uuid,
        update.mode,
        update.primary_book_file_id,
    )
    .await
    {
        Ok(_) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            (axum::http::StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => internal("confirm_cross_format", e),
    }
}

/// `DELETE /api/books/{uuid}/cross-format-link` — turn sync off; 204
/// whether or not a link existed (idempotent), 404 only for an unknown
/// book. Mirrors `rpc_unlink_cross_format`.
pub(super) async fn delete_cross_format_link(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::cross_format::delete_link(&state.pool, user.id, &uuid).await {
        Ok(_) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            (axum::http::StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => internal("unlink_cross_format", e),
    }
}
