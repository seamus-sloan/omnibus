//! `/api/account/hidden-formats` — the caller's own hidden-formats
//! preference (which formats the landing All Books view excludes for them).
//! Mirrors the web server function `rpc_set_hidden_formats`; the read side
//! rides `GET /api/auth/me` as `UserSummary::hidden_formats`.

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

#[cfg(test)]
mod tests;
