//! Metadata-provider catalog REST handlers — mobile- and web-facing
//! (`GET /api/metadata/providers`). Any authenticated user may read it; it
//! carries no key material, only `configured: bool`.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db as db;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// The provider catalog for this instance: identity, whether each provider
/// is usable right now, and what each can answer. Drives a future
/// provider-filter UI; today it lets any client enumerate sources without
/// hardcoding a `match` on `MetadataProvider`.
///
/// Keys resolve settings-over-env through `db::provider_keys` — the same
/// resolution the check-in scan ladder uses — so a saved Settings key and an
/// env-var key report `configured: true` identically.
pub(super) async fn get_providers(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::provider_keys(&state.pool).await {
        Ok(keys) => Json(db::catalog(&db::MetadataLookupConfig::live(keys))).into_response(),
        Err(e) => internal("metadata provider catalog", e),
    }
}

#[cfg(test)]
mod tests;
