//! Admin book-merge REST endpoints: absorb one book into another and
//! reverse a recorded merge. Thin HTTP shells over `db::merge_books` /
//! `db::undo_merge` — the same helpers the web-facing
//! `/api/rpc/merge-books*` server functions call.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use omnibus_shared::{MergeBooksRequest, MergeBooksResult, UndoMergeRequest, UndoMergeResult};

use omnibus_db as db;

use super::{internal, AppState};
use crate::auth::AdminUser;

#[cfg(test)]
mod tests;

/// `POST /api/books/merge` — merge `source_uuid` into `target_uuid`: the
/// target absorbs the source's files, links, identifiers, and per-reader
/// state, and the source row disappears. Returns the `merge_log` id (the
/// undo handle) and the surviving uuid. Mirrors `rpc_merge_books` in
/// `omnibus_frontend::rpc::books`, whose `AdminUser` gate this repeats —
/// the merge semantics live in `db::merge_books`, but there is no shared
/// gate helper between the two crates, so changing one side's gate means
/// changing the other.
pub(super) async fn post_merge_books(
    admin: AdminUser,
    State(state): State<AppState>,
    Json(req): Json<MergeBooksRequest>,
) -> Response {
    match db::merge_books(
        &state.pool,
        &req.source_uuid,
        &req.target_uuid,
        Some(admin.0.id),
    )
    .await
    {
        Ok(out) => Json(MergeBooksResult {
            merge_log_id: out.merge_log_id,
            target_uuid: out.target_uuid,
        })
        .into_response(),
        Err(e @ db::MergeError::BookNotFound(_)) => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e @ db::MergeError::SameBook) => {
            (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response()
        }
        Err(e) => internal("merge books", e),
    }
}

/// `POST /api/books/merge/undo` — reverse a merge recorded in
/// `merge_log`; returns the restored (source) book's uuid. Mirrors
/// `rpc_undo_merge`, with the same gate-parity note as
/// [`post_merge_books`].
pub(super) async fn post_undo_merge(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(req): Json<UndoMergeRequest>,
) -> Response {
    match db::undo_merge(&state.pool, req.merge_log_id).await {
        Ok(uuid) => Json(UndoMergeResult {
            restored_uuid: uuid,
        })
        .into_response(),
        Err(e @ db::MergeError::LogNotFound) => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        // Both are "the state moved on since the merge", which is a conflict
        // the caller can act on rather than an internal failure.
        Err(e @ (db::MergeError::AlreadyUndone | db::MergeError::UndoConflict(_))) => {
            (StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(e) => internal("undo merge", e),
    }
}
