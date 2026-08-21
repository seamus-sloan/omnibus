//! Metadata-provider REST handlers — mobile-facing; web hits the analogous
//! `/api/rpc/metadata/*` server fns, which mirror these gates. The catalog
//! (`GET /api/metadata/providers`) is readable by any authenticated user and
//! carries no key material, only `configured: bool`; the fan-out edition
//! search (`POST /api/metadata/editions/search`) is `can_edit`-gated, since it
//! makes outbound provider calls.

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

/// The provider config for this instance: saved settings keys win over the
/// `GOOGLE_BOOKS_API_KEY` / `HARDCOVER_API_KEY` env vars — the same
/// resolution the check-in scan ladder uses, so a saved key and an env key
/// report `configured: true` identically.
async fn provider_config(state: &AppState) -> Result<db::MetadataLookupConfig, db::SettingsError> {
    Ok(db::MetadataLookupConfig::live(
        db::provider_keys(&state.pool).await?,
    ))
}

/// The provider catalog for this instance: identity, whether each provider
/// is usable right now, and what each can answer. Drives a future
/// provider-filter UI; today it lets any client enumerate sources without
/// hardcoding a `match` on `MetadataProvider`.
pub(super) async fn get_providers(_user: AuthUser, State(state): State<AppState>) -> Response {
    match provider_config(&state).await {
        Ok(config) => Json(db::catalog(&config)).into_response(),
        Err(e) => internal("metadata provider catalog", e),
    }
}

/// Fan an edition search out across the providers, keeping every source's
/// candidates attributed and un-collapsed for the metadata editor's picker.
///
/// Always 200 once the request is well-formed and the caller may edit: a
/// provider that fails reports `Failed` on its own status row rather than
/// failing the search, so one source's outage never costs the reader the
/// others' results. 400 for a blank/oversized query or an empty provider list,
/// 403 without edit permission — this is the one metadata read that spends
/// outbound provider calls, so it is gated like the overrides *write*.
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
    let config = match provider_config(&state).await {
        Ok(c) => c,
        Err(e) => return internal("edition_search_provider_keys", e),
    };
    let found =
        db::search_all_providers(&config, &search_query(&req), req.providers.as_deref()).await;
    Json(found).into_response()
}

/// Turn a request into the query the providers are actually asked.
///
/// The structured fields win when the caller sent them; free text falls back
/// to a title-only query, which is the only honest reading of a string a
/// reader typed. Mirrors `omnibus_frontend::rpc::metadata_search`'s own
/// helper — the two front doors must agree on what a request means.
fn search_query(req: &EditionSearchRequest) -> db::SearchQuery {
    // Built first, then checked for content — the structured fields are
    // *cleaned* (blanks trimmed to absent, an unusable ISBN discarded), so
    // branching on `is_some()` alone would let `{"query":"Dune","title":"  "}`
    // throw the reader's query away and search for nothing.
    let structured = db::SearchQuery::new(
        req.title.as_deref(),
        req.author.as_deref(),
        req.isbn.as_deref(),
    );
    if structured.is_empty() {
        return db::SearchQuery::from_text(&req.query);
    }
    // An ISBN or an author with no title still needs something to rank
    // against, and the free text is what the reader actually sees in the box.
    db::SearchQuery {
        title: structured
            .title
            .clone()
            .or_else(|| db::SearchQuery::from_text(&req.query).title),
        ..structured
    }
}

#[cfg(test)]
mod tests;
