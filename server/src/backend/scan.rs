//! Physical Check-In scan REST handlers for the mobile client (`/api/scan/*`):
//! resolve a scanned or typed ISBN down the matching ladder, then check in a
//! physical copy, add a physical-only book, or wishlist one. Any authenticated
//! user may act — physical ownership is library-wide. Web clients use the
//! analogous `/api/rpc/scan/*` server functions.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, MetadataLookupError, PhysicalError, ScanError};
use omnibus_shared::{
    AddPhysicalOnlyRequest, BookRef, CheckInRequest, ResolveMetaRequest, ResolveRequest,
    ScanSearchRequest, ScanSearchResponse, WishlistAddRequest,
};
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::{AdminUser, AuthUser};

/// The provider ladder's config for this instance: saved settings keys win
/// over the `GOOGLE_BOOKS_API_KEY` / `HARDCOVER_API_KEY` env vars, and a
/// provider with no key is simply one the ladder skips.
async fn provider_config(state: &AppState) -> Result<db::MetadataLookupConfig, db::SettingsError> {
    Ok(db::MetadataLookupConfig::live(
        db::provider_keys(&state.pool).await?,
    ))
}

/// Map a scan-flow error to a response: user-actionable cases become 400/404,
/// an unreachable metadata provider becomes 503, DB failures become 500.
fn scan_error(context: &'static str, e: ScanError) -> Response {
    match e {
        ScanError::Isbn(inner) => (StatusCode::BAD_REQUEST, inner.to_string()).into_response(),
        ScanError::MissingWishlistTarget => {
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
        ScanError::Physical(PhysicalError::BookNotFound) => {
            (StatusCode::NOT_FOUND, "book not found").into_response()
        }
        // Both providers being down is an outage, not a bug in Omnibus: 503
        // with the cause logged, so the flow says "try again later" instead of
        // "internal server error".
        ScanError::Lookup(inner @ MetadataLookupError::Provider(_)) => {
            tracing::warn!(context, error = ?inner, "metadata provider unavailable");
            (StatusCode::SERVICE_UNAVAILABLE, inner.to_string()).into_response()
        }
        other => internal(context, other),
    }
}

/// Resolve an ISBN. Always 200 with a `ScanOutcome` (including `Unresolved`);
/// 400 only for an invalid ISBN.
pub(super) async fn post_resolve(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ResolveRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let config = match provider_config(&state).await {
        Ok(c) => c,
        Err(e) => return internal("scan_resolve_provider_keys", e),
    };
    match db::resolve_scan(&state.pool, user.id, &req.isbn, &config).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => scan_error("scan_resolve", e),
    }
}

/// Search the providers by title text — the fallback when an ISBN resolves to
/// `Unresolved`. Always 200 with a (possibly empty) candidate list; 400 for a
/// blank/oversized query, 503 when both providers are down.
pub(super) async fn post_search(
    _user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ScanSearchRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let config = match provider_config(&state).await {
        Ok(c) => c,
        Err(e) => return internal("scan_search_provider_keys", e),
    };
    match db::search_provider_by_title(&config, req.query.trim()).await {
        Ok(results) => Json(ScanSearchResponse { results }).into_response(),
        Err(e) => scan_error("scan_search", e.into()),
    }
}

/// Resolve a picked title-search candidate against the library — the ladder's
/// library rungs applied to metadata already in hand, no provider ISBN lookup.
pub(super) async fn post_resolve_meta(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ResolveMetaRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let config = match provider_config(&state).await {
        Ok(c) => c,
        Err(e) => return internal("scan_resolve_meta_provider_keys", e),
    };
    match db::resolve_meta(&state.pool, user.id, &req.meta, &config).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => scan_error("scan_resolve_meta", e),
    }
}

/// Whether a server-wide Google Books key is configured. Any authenticated
/// user may read this non-sensitive flag; it drives the check-in scan
/// screen's provider note (mirrors `backend::summary::get_hardcover_configured`).
/// Distinct from the admin-only [`get_google_books_key`], which also carries
/// a masked key preview.
pub(super) async fn get_google_books_configured(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Response {
    match db::google_books_key_status(&state.pool).await {
        Ok(status) => Json(status.configured).into_response(),
        Err(e) => internal("google books key status", e),
    }
}

// This key's REST handlers mirror the Hardcover handlers in
// `backend/suggestions.rs`; kept per feature module rather than unified — the
// shared logic already lives once in db's `SecretKeySpec`.
/// Admin-only: masked status of the server-wide Google Books key.
pub(super) async fn get_google_books_key(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Response {
    match db::google_books_key_status(&state.pool).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal("read google books key", e),
    }
}

/// Body for `POST /api/google-books-key`. A `null`/absent/blank key clears it.
#[derive(Debug, Deserialize)]
pub(super) struct SetGoogleBooksKey {
    #[serde(default)]
    key: Option<String>,
}

/// Admin-only: save or clear the Google Books key; returns the new masked
/// status. A value over `GOOGLE_BOOKS_API_KEY_MAX_LEN` returns 422 with the
/// typed validation message so an admin sees a per-case error rather than a 500.
pub(super) async fn post_google_books_key(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<SetGoogleBooksKey>,
) -> Response {
    match db::set_google_books_api_key(&state.pool, body.key.as_deref()).await {
        Ok(()) => {}
        Err(db::SettingsError::Validation(msg)) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
        }
        Err(e) => return internal("save google books key", e),
    }
    match db::google_books_key_status(&state.pool).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal("read google books key", e),
    }
}

/// Check in a physical copy of a library book (fulfills every user's wishlist).
pub(super) async fn post_check_in(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CheckInRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::check_in_copy(
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
        Err(e) => scan_error("scan_check_in", e),
    }
}

/// Add a physical-only book from resolved external metadata.
pub(super) async fn post_add_physical_only(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<AddPhysicalOnlyRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
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
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
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
