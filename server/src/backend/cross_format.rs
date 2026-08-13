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

/// One error mapping for every handler in this module: 404 for an unknown
/// book, 409 for declaration refusals the client can act on, 500 for the
/// rest.
fn error_response(op: &'static str, e: db::cross_format::CrossFormatError) -> Response {
    use db::cross_format::CrossFormatError as E;
    match e {
        E::BookNotFound => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        e @ (E::AudioSetMismatch | E::LinkRequired | E::CounterpartMissing) => {
            (axum::http::StatusCode::CONFLICT, e.to_string()).into_response()
        }
        E::Sqlx(e) => internal(op, e),
    }
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
        Err(e) => error_response("cross_format_resume", e),
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
        Err(e) => error_response("get_alignment", e),
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
            Err(e) => return error_response("set_audio_order", e),
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
        Err(e) => error_response("confirm_cross_format", e),
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
        Err(e) => error_response("unlink_cross_format", e),
    }
}

/// `POST /api/books/{uuid}/sync-point` — a "synced here" declaration: the
/// path uuid is authoritative; the body names the declaring surface's own
/// position and the counterpart comes from its stored row. Turns follow
/// mode on. 409 when the alignment must be confirmed first (multi-file,
/// no link), when the audio set went stale, or when the counterpart has
/// no position to pair with.
pub(super) async fn post_sync_point(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(mut decl): Json<omnibus_shared::cross_format::DeclareSyncPoint>,
) -> Response {
    decl.book_uuid = uuid;
    if let Err(msg) = decl.validate() {
        return (axum::http::StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
    }
    match db::cross_format::declare_sync_point(&state.pool, user.id, &decl).await {
        Ok(_) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => error_response("declare_sync_point", e),
    }
}

/// `POST /api/books/{uuid}/cross-format-follow` — flip follow mode on an
/// existing link. 409 when no link exists to flip.
pub(super) async fn post_cross_format_follow(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(body): Json<omnibus_shared::cross_format::SetFollowMode>,
) -> Response {
    match db::cross_format::set_follow(&state.pool, user.id, &uuid, body.enabled).await {
        Ok(true) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            axum::http::StatusCode::CONFLICT,
            "confirm the alignment first",
        )
            .into_response(),
        Err(e) => error_response("set_follow", e),
    }
}
