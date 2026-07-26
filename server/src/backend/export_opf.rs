//! Per-book OPF export handler (REST, mobile-facing).
//!
//! An edit-permitted user `POST`s against a book's uuid to write its
//! `metadata.opf` sidecar; the response reports the path and whether an
//! existing sidecar was backed up. Mounted on the REST router in
//! [`super::rest_router`].

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::OpfExportResult;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Export a book's merged metadata to its `metadata.opf` sidecar. Requires
/// `can_edit` or admin.
pub(super) async fn post_export_opf(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (StatusCode::FORBIDDEN, "edit permission required").into_response();
    }
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    match db::export_opf(&state.pool, id).await {
        Ok(export) => Json(OpfExportResult {
            path: export.path.display().to_string(),
            backed_up: export.backed_up,
        })
        .into_response(),
        Err(db::OpfExportError::BookNotFound(_)) => {
            (StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(db::OpfExportError::NoEpubFile(_)) => (
            StatusCode::CONFLICT,
            "book has no EPUB file to export next to",
        )
            .into_response(),
        Err(e) => internal("export_opf", e),
    }
}

#[cfg(test)]
mod tests;
