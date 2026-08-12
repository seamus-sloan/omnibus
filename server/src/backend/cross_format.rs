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
