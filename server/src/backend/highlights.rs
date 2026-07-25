//! Highlight annotation REST handlers for the mobile client (`/api/highlights*`).
//!
//! Web clients use the `/api/rpc/highlights/*` server functions defined in
//! `omnibus_frontend::rpc`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, highlights::HighlightError};
use omnibus_shared::{CreateHighlight, HighlightColor, UpdateHighlightNote};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Create a highlight annotation on a book.
pub(super) async fn post_highlight(
    user: AuthUser,
    State(state): State<AppState>,
    Json(input): Json<CreateHighlight>,
) -> Response {
    if let Err(msg) = input.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::highlights::create_highlight(&state.pool, user.id, &input).await {
        Ok(h) => Json(h).into_response(),
        Err(HighlightError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        // NotFound on create means the just-inserted row couldn't be re-read —
        // a server invariant break, not a missing client resource. Surface 500.
        Err(e @ HighlightError::NotFound) => internal("create_highlight", e),
        Err(HighlightError::Sqlx(e)) => internal("create_highlight", e),
    }
}

/// List all highlights for a user on a specific book.
pub(super) async fn get_highlights(
    user: AuthUser,
    State(state): State<AppState>,
    Path(book_uuid): Path<String>,
) -> Response {
    match db::highlights::list_highlights(&state.pool, user.id, &book_uuid).await {
        Ok(list) => Json(list).into_response(),
        Err(HighlightError::Sqlx(e)) => internal("list_highlights", e),
        Err(e) => internal("list_highlights", e),
    }
}

/// Resolve a `{id}` path segment to a numeric highlight id.
///
/// The segment is either the server's own row id or the uuid the creating
/// device minted (migration 0049). Both are valid handles: an offline client
/// queues an edit or a delete against its own uuid long before it has heard
/// what row id the create was assigned, and replaying that op has to land on
/// the same annotation. A uuid can't parse as an `i64`, so the two namespaces
/// can't collide.
async fn resolve_id(state: &AppState, user_id: i64, raw: &str) -> Result<i64, HighlightError> {
    if let Ok(id) = raw.parse::<i64>() {
        return Ok(id);
    }
    db::highlights::highlight_id_for_client_id(&state.pool, user_id, raw)
        .await?
        .ok_or(HighlightError::NotFound)
}

#[derive(serde::Deserialize)]
pub(super) struct PatchColor {
    color: HighlightColor,
}

/// Update the color of an existing highlight.
pub(super) async fn patch_highlight_color(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchColor>,
) -> Response {
    let id = match resolve_id(&state, user.id, &id).await {
        Ok(id) => id,
        Err(HighlightError::NotFound) => {
            return (axum::http::StatusCode::NOT_FOUND, "highlight not found").into_response()
        }
        Err(e) => return internal("update_highlight_color", e),
    };
    match db::highlights::update_highlight_color(&state.pool, user.id, id, body.color).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(HighlightError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "highlight not found").into_response()
        }
        Err(HighlightError::Sqlx(e)) => internal("update_highlight_color", e),
        Err(e) => internal("update_highlight_color", e),
    }
}

/// Update the note on an existing highlight.
pub(super) async fn patch_highlight_note(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateHighlightNote>,
) -> Response {
    if let Err(msg) = body.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    let id = match resolve_id(&state, user.id, &id).await {
        Ok(id) => id,
        Err(HighlightError::NotFound) => {
            return (axum::http::StatusCode::NOT_FOUND, "highlight not found").into_response()
        }
        Err(e) => return internal("update_highlight_note", e),
    };
    match db::highlights::update_highlight_note(&state.pool, user.id, id, body.note.as_deref())
        .await
    {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(HighlightError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "highlight not found").into_response()
        }
        Err(HighlightError::Sqlx(e)) => internal("update_highlight_note", e),
        Err(e) => internal("update_highlight_note", e),
    }
}

/// Delete a highlight by id, scoped to the owning user.
pub(super) async fn delete_highlight(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let id = match resolve_id(&state, user.id, &id).await {
        Ok(id) => id,
        Err(HighlightError::NotFound) => {
            return (axum::http::StatusCode::NOT_FOUND, "highlight not found").into_response()
        }
        Err(e) => return internal("delete_highlight", e),
    };
    match db::highlights::delete_highlight(&state.pool, user.id, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(HighlightError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "highlight not found").into_response()
        }
        Err(HighlightError::Sqlx(e)) => internal("delete_highlight", e),
        Err(e) => internal("delete_highlight", e),
    }
}

#[cfg(test)]
mod tests;
