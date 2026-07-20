//! Physical Check-In scan REST handlers for the mobile client (`/api/scan/*`).
//!
//! Resolve a scanned/typed ISBN down the matching ladder, then check in a
//! physical copy, add a physical-only book, or wishlist a book. Any
//! authenticated user may act — physical ownership is library-wide. Web clients
//! use the analogous `/api/rpc/scan/*` server functions in
//! `omnibus_frontend::rpc`.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, PhysicalError, ScanError};
use omnibus_shared::{
    AddPhysicalOnlyRequest, BookRef, CheckInRequest, ResolveRequest, WishlistAddRequest,
};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Map a scan-flow error to a response: user-actionable cases become 400/404,
/// provider/DB failures become 500.
fn scan_error(context: &'static str, e: ScanError) -> Response {
    match e {
        ScanError::Isbn(inner) => (StatusCode::BAD_REQUEST, inner.to_string()).into_response(),
        ScanError::MissingWishlistTarget => {
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
        ScanError::Physical(PhysicalError::BookNotFound) => {
            (StatusCode::NOT_FOUND, "book not found").into_response()
        }
        other => internal(context, other),
    }
}

/// Resolve an ISBN. Always 200 with a `ScanOutcome` (including `Unresolved`);
/// 400 only for an invalid ISBN.
pub(super) async fn post_resolve(
    _user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ResolveRequest>,
) -> Response {
    let config = db::MetadataLookupConfig::live();
    match db::resolve_scan(&state.pool, &req.isbn, &config).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => scan_error("scan_resolve", e),
    }
}

/// Check in a physical copy of a library book (fulfills every user's wishlist).
pub(super) async fn post_check_in(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CheckInRequest>,
) -> Response {
    match db::add_physical_copy(
        &state.pool,
        &req.book_uuid,
        req.isbn.as_deref(),
        Some(user.id),
        req.note.as_deref(),
    )
    .await
    {
        Ok(copy) => Json(BookRef {
            book_uuid: copy.book_uuid,
        })
        .into_response(),
        Err(e) => scan_error("scan_check_in", e.into()),
    }
}

/// Add a physical-only book from resolved external metadata.
pub(super) async fn post_add_physical_only(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<AddPhysicalOnlyRequest>,
) -> Response {
    match db::add_physical_only(&state.pool, &req.meta, req.note.as_deref(), Some(user.id)).await {
        Ok(book_uuid) => Json(BookRef { book_uuid }).into_response(),
        Err(e) => scan_error("scan_add_physical_only", e),
    }
}

/// Add a book to the caller's physical wishlist.
pub(super) async fn post_wishlist_add(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<WishlistAddRequest>,
) -> Response {
    match db::wishlist_add(
        &state.pool,
        user.id,
        req.book_uuid.as_deref(),
        req.meta.as_ref(),
        req.source,
    )
    .await
    {
        Ok(book_uuid) => Json(BookRef { book_uuid }).into_response(),
        Err(e) => scan_error("scan_wishlist_add", e),
    }
}

#[cfg(test)]
mod tests;
