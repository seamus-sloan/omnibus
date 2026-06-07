//! `/api/authors/*` handlers.
//!
//! Cookie-gated reads that return the authors index and per-author detail.
//! Detail reads also enqueue a background `ResolveAuthorPhoto` task when
//! the author has no cached photo yet.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, worker::Task};

use super::{internal, AppState};
use crate::auth::AuthUser;

pub(super) async fn get_author_by_id(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_author(&state.pool, id).await {
        Ok(Some(author)) => {
            // F1.11: queue a background resolution when the author has no
            // `author_photos` row yet so first-time visits trigger Open
            // Library resolution. A subsequent visit picks up the resolved
            // photo (or the `letter` negative-cache marker).
            if !author.has_photo {
                match db::author_photo_status(&state.pool, id).await {
                    Ok(None) => {
                        state
                            .worker
                            .post(Task::ResolveAuthorPhoto { author_id: id });
                    }
                    Ok(Some(_)) => {}
                    Err(e) => {
                        // Don't fail the read — just skip the queue and log.
                        tracing::warn!(
                            author_id = id,
                            error = %e,
                            "author_photo_status check failed; skipping autoresolution"
                        );
                    }
                }
            }
            Json(author).into_response()
        }
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read author", error),
    }
}

/// `/authors` index. Returns every author across both ebook and audiobook
/// libraries with a book count and optional accent.
pub(super) async fn get_authors(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    let paths = db::collect_paths(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    );
    match db::list_authors(&state.pool, &paths).await {
        Ok(authors) => Json(authors).into_response(),
        Err(e) => internal("list authors", e),
    }
}

#[cfg(test)]
mod tests;
