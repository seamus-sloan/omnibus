//! Community-rating handlers (`/api/ebooks/{uuid}/external-ratings`).
//!
//! The read is open to any authenticated user; the refresh is `can_edit`-gated
//! because it spends outbound provider calls, exactly like the edition search.
//! These are the provider-authored scores shown *beside* the reader's own star
//! rating, whose handlers live in [`super::ratings`].

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, external_ratings::ExternalRatingsError};
use omnibus_shared::external_ratings::RefreshRatingsRequest;
use omnibus_shared::isbn::normalize_isbn;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Every community rating stored for a book, attributed to its source.
/// `200 []` for a book with none — and for an unknown uuid, matching
/// `get_other_ratings`: the detail page asks for this alongside the book
/// itself, which is where a missing book is reported.
pub(super) async fn get_external_ratings(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::external_ratings::list_ratings(&state.pool, &uuid).await {
        Ok(ratings) => Json(ratings).into_response(),
        Err(e) => internal("list_external_ratings", e),
    }
}

/// Re-ask every configured provider for its community rating of `isbn13` and
/// store what comes back, returning the book's ratings as they now stand.
///
/// Called when a candidate is applied to the book — a rating is fetched for a
/// book somebody chose, never for every edition a search scrolled past. 400 on
/// an ISBN that fails validation, 403 without edit permission, 404 for a book
/// the server has never indexed. A provider that fails contributes no row and
/// never fails the request.
pub(super) async fn post_external_ratings(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(req): Json<RefreshRatingsRequest>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (StatusCode::FORBIDDEN, "edit permission required").into_response();
    }
    let isbn13 = match normalize_isbn(&req.isbn13) {
        Ok(isbn) => isbn,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let config = match db::provider_keys(&state.pool).await {
        Ok(keys) => db::MetadataLookupConfig::live(keys),
        Err(e) => return internal("external_ratings_provider_keys", e),
    };
    match db::external_ratings::refresh_ratings(&state.pool, &config, &uuid, &isbn13).await {
        Ok(ratings) => Json(ratings).into_response(),
        Err(ExternalRatingsError::BookNotFound) => {
            (StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(ExternalRatingsError::Sqlx(e)) => internal("refresh_external_ratings", e),
    }
}

#[cfg(test)]
mod tests;
