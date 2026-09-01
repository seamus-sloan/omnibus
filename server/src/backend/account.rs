//! `/api/account/*` — the caller's own reading preferences: which formats the
//! landing All Books view excludes for them, and whether their book detail
//! page uses the snap-stop marquee. Mirrors the web server functions in
//! `frontend::rpc::account`; both read sides ride `GET /api/auth/me` as
//! `UserSummary` fields.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Body for `POST /api/account/hidden-formats`. An empty/absent list clears
/// the preference.
#[derive(Debug, Deserialize)]
pub(super) struct SetHiddenFormats {
    #[serde(default)]
    formats: Vec<String>,
}

/// Replace the authenticated user's hidden-formats list. 422 on a token that
/// isn't a plausible format name or an oversized list.
pub(super) async fn post_hidden_formats(
    user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<SetHiddenFormats>,
) -> Response {
    match db::auth::set_hidden_formats(&state.pool, user.id, &body.formats).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(db::auth::AuthError::Validation(msg)) => {
            (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response()
        }
        Err(e) => internal("set hidden formats", e),
    }
}

/// Body for `POST /api/account/book-detail-scroll-stops`.
#[derive(Debug, Deserialize)]
pub(super) struct SetBookDetailScrollStops {
    enabled: bool,
}

/// Set the authenticated user's book-detail scroll-stops preference.
pub(super) async fn post_book_detail_scroll_stops(
    user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<SetBookDetailScrollStops>,
) -> Response {
    match db::auth::set_book_detail_scroll_stops(&state.pool, user.id, body.enabled).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => internal("set book detail scroll stops", e),
    }
}

#[cfg(test)]
mod tests;
