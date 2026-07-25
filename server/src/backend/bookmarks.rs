//! Bookmark REST handlers for the mobile client (`/api/bookmarks*`).
//!
//! Web clients use the `/api/rpc/bookmarks/*` server functions defined in
//! `omnibus_frontend::rpc`. One model serves both the audiobook player and the
//! reader — `position` is seconds-as-string or an EPUB CFI respectively.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, bookmarks::BookmarkError};
use omnibus_shared::{CreateBookmark, UpdateBookmark};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Create a bookmark on a book.
pub(super) async fn post_bookmark(
    user: AuthUser,
    State(state): State<AppState>,
    Json(input): Json<CreateBookmark>,
) -> Response {
    if let Err(msg) = input.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::bookmarks::create_bookmark(&state.pool, user.id, &input).await {
        Ok(b) => Json(b).into_response(),
        Err(BookmarkError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        // NotFound on create means the just-inserted row couldn't be re-read —
        // a server invariant break, not a missing client resource. Surface 500.
        Err(e @ BookmarkError::NotFound) => internal("create_bookmark", e),
        Err(BookmarkError::Sqlx(e)) => internal("create_bookmark", e),
    }
}

/// List all bookmarks for a user on a specific book.
pub(super) async fn get_bookmarks(
    user: AuthUser,
    State(state): State<AppState>,
    Path(book_uuid): Path<String>,
) -> Response {
    match db::bookmarks::list_bookmarks(&state.pool, user.id, &book_uuid).await {
        Ok(list) => Json(list).into_response(),
        Err(BookmarkError::Sqlx(e)) => internal("list_bookmarks", e),
        Err(e) => internal("list_bookmarks", e),
    }
}

/// Resolve a `{id}` path segment to a numeric bookmark id.
///
/// Accepts either the server's row id or the uuid the creating device minted
/// (migration 0049), so an offline client can address a bookmark it created
/// but has not yet synced. See `highlights::resolve_id` for the full rationale.
async fn resolve_id(state: &AppState, user_id: i64, raw: &str) -> Result<i64, BookmarkError> {
    if let Ok(id) = raw.parse::<i64>() {
        return Ok(id);
    }
    db::bookmarks::bookmark_id_for_client_id(&state.pool, user_id, raw)
        .await?
        .ok_or(BookmarkError::NotFound)
}

/// Update the title/note on an existing bookmark.
pub(super) async fn put_bookmark(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBookmark>,
) -> Response {
    if let Err(msg) = body.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    let id = match resolve_id(&state, user.id, &id).await {
        Ok(id) => id,
        Err(BookmarkError::NotFound) => {
            return (axum::http::StatusCode::NOT_FOUND, "bookmark not found").into_response()
        }
        Err(e) => return internal("update_bookmark", e),
    };
    match db::bookmarks::update_bookmark(&state.pool, user.id, id, body.title.as_deref()).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(BookmarkError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "bookmark not found").into_response()
        }
        Err(BookmarkError::Sqlx(e)) => internal("update_bookmark", e),
        Err(e) => internal("update_bookmark", e),
    }
}

/// Delete a bookmark by id, scoped to the owning user.
pub(super) async fn delete_bookmark(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let id = match resolve_id(&state, user.id, &id).await {
        Ok(id) => id,
        Err(BookmarkError::NotFound) => {
            return (axum::http::StatusCode::NOT_FOUND, "bookmark not found").into_response()
        }
        Err(e) => return internal("delete_bookmark", e),
    };
    match db::bookmarks::delete_bookmark(&state.pool, user.id, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(BookmarkError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "bookmark not found").into_response()
        }
        Err(BookmarkError::Sqlx(e)) => internal("delete_bookmark", e),
        Err(e) => internal("delete_bookmark", e),
    }
}

#[cfg(test)]
mod tests;
