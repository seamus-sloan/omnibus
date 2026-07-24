//! Physical-collection REST handlers for the mobile client (`/api/physical/*`):
//! a book's library-wide copies, the caller's wishlist entry, and fileless-book
//! removal. Check-in lives in [`super::scan`]; web uses the `/api/rpc/physical*`
//! server functions.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, PhysicalError};
use omnibus_shared::physical::{UpdateCopyNoteRequest, WishlistSource};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Map a data-layer error to its HTTP shape. `BookHasFiles` is a 409: the
/// request is well-formed but conflicts with the book's current state.
fn physical_error(context: &'static str, e: PhysicalError) -> Response {
    match e {
        PhysicalError::BookNotFound => (StatusCode::NOT_FOUND, "book not found").into_response(),
        PhysicalError::CopyNotFound => {
            (StatusCode::NOT_FOUND, "physical copy not found").into_response()
        }
        PhysicalError::BookHasFiles => (
            StatusCode::CONFLICT,
            "book still has files; remove them first",
        )
            .into_response(),
        other => internal(context, other),
    }
}

/// Reject a caller without `can_edit`. Physical copies are library-wide, so
/// editing one changes what every user sees — same gate as metadata overrides.
fn require_edit(user: &AuthUser) -> Option<Response> {
    (!user.is_admin && !user.can_edit)
        .then(|| (StatusCode::FORBIDDEN, "edit permission required").into_response())
}

/// List a book's physical copies, oldest check-in first. An unknown uuid has no
/// copies, so it returns `200 []` rather than 404.
pub(super) async fn get_copies(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::list_physical_copies(&state.pool, &uuid).await {
        Ok(copies) => Json(copies).into_response(),
        Err(e) => physical_error("list_physical_copies", e),
    }
}

/// Replace a copy's free-text note. A blank note clears it.
pub(super) async fn patch_copy_note(
    user: AuthUser,
    State(state): State<AppState>,
    Path(copy_id): Path<i64>,
    Json(req): Json<UpdateCopyNoteRequest>,
) -> Response {
    if let Some(denied) = require_edit(&user) {
        return denied;
    }
    match db::update_physical_copy_note(&state.pool, copy_id, req.note.as_deref()).await {
        Ok(copy) => Json(copy).into_response(),
        Err(e) => physical_error("update_physical_copy_note", e),
    }
}

/// Delete one physical copy ("I sold it"). 404 when the id is unknown.
pub(super) async fn delete_copy(
    user: AuthUser,
    State(state): State<AppState>,
    Path(copy_id): Path<i64>,
) -> Response {
    if let Some(denied) = require_edit(&user) {
        return denied;
    }
    match db::delete_physical_copy(&state.pool, copy_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => physical_error("delete_physical_copy", e),
    }
}

/// The caller's wishlist entry for a book, or `null` when not wishlisted.
pub(super) async fn get_wishlist_entry(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::get_wishlist_entry(&state.pool, user.id, &uuid).await {
        Ok(entry) => Json(entry).into_response(),
        Err(e) => physical_error("get_wishlist_entry", e),
    }
}

/// Add a book already in the library to the caller's wishlist from its detail
/// page. Idempotent — a second add returns the existing entry unchanged. (The
/// scan flow's `/api/scan/wishlist` is the other door, for a book the library
/// doesn't hold yet; that one carries external metadata, this one a bare uuid.)
pub(super) async fn post_wishlist_entry(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::add_wishlist_entry(&state.pool, user.id, &uuid, WishlistSource::Detail).await {
        Ok(entry) => Json(entry).into_response(),
        Err(e) => physical_error("add_wishlist_entry", e),
    }
}

/// Remove a book from the caller's wishlist. Idempotent — removing an absent
/// entry still returns 204, since the desired end state already holds.
pub(super) async fn delete_wishlist_entry(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::remove_wishlist_entry(&state.pool, user.id, &uuid).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => physical_error("remove_wishlist_entry", e),
    }
}

/// Delete a fileless book outright — the "remove it entirely" branch of the
/// last-copy prompt. 409 when the book still has digital files.
pub(super) async fn delete_book(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    if let Some(denied) = require_edit(&user) {
        return denied;
    }
    match db::delete_fileless_book(&state.pool, &uuid).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => physical_error("delete_fileless_book", e),
    }
}

#[cfg(test)]
mod tests;
