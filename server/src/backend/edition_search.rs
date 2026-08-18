//! Fan-out edition search REST handler (`POST /api/metadata/editions/search`).
//!
//! Asks every configured provider at once and answers with attributed
//! candidates plus a per-source status. Edit-gated like the overrides write,
//! since it triggers outbound provider calls.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db as db;
use omnibus_shared::metadata_lookup::EditionSearchRequest;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Search the metadata providers for candidate editions.
///
/// Always 200 once the request validates — a provider that fails is a
/// `Failed` row in `sources`, never a failed request, so the picker can show
/// what did answer. 400 for a blank/oversized query, 403 without edit
/// permission.
pub(super) async fn post_edition_search(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<EditionSearchRequest>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (StatusCode::FORBIDDEN, "edit permission required").into_response();
    }
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let keys = match db::provider_keys(&state.pool).await {
        Ok(keys) => keys,
        Err(e) => return internal("edition_search_provider_keys", e),
    };
    let config = db::MetadataLookupConfig::live(keys);
    let response =
        db::search_all_providers(&config, req.query.trim(), req.providers.as_deref()).await;
    Json(response).into_response()
}

#[cfg(test)]
mod tests;
