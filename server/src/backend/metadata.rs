//! Metadata-provider REST handlers — mobile-facing; web hits the analogous
//! `/api/rpc/metadata/*` server fns, which mirror these gates. The catalog is
//! readable by any authenticated user and carries no key material; the edition
//! search and hydrate are `can_edit`-gated, since they spend outbound provider
//! calls.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db as db;
use omnibus_shared::metadata_lookup::{EditionHydrateRequest, EditionSearchRequest};

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

/// Re-fetch one selected candidate from the provider that offered it.
///
/// The picker's second call: a search hit is thinner than the provider's own
/// record, so selecting one is worth a round trip. Answers `null` when that
/// provider no longer knows the candidate — a clean miss the caller absorbs by
/// keeping the list row it already has, never by blanking it.
///
/// 400 for a blank or oversized handle, 403 without edit permission (the same
/// gate the search carries, for the same reason), and 502 when the provider
/// itself could not be reached — which is a failure, unlike the miss above.
pub(super) async fn post_edition_hydrate(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<EditionHydrateRequest>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (StatusCode::FORBIDDEN, "edit permission required").into_response();
    }
    if let Err(msg) = req.validate() {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let config = match provider_config(&state).await {
        Ok(c) => c,
        Err(e) => return internal("edition_hydrate_provider_keys", e),
    };
    let found = db::hydrate_edition(
        &config,
        req.source,
        req.provider_ref.trim(),
        req.isbn13
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
    )
    .await;
    match found {
        Ok(edition) => Json(edition).into_response(),
        // Logged with `?e`, not `{e:#}`: thiserror lowers a literal
        // `#[error("…")]` to a `write_str`, which ignores the alternate flag,
        // so `{e:#}` would record the same fixed sentence the caller already
        // got and the provider's own cause would reach nothing. `Debug` walks
        // the chain. Safe to log in full — `providers::http::strip_url` has
        // already removed the request URL, and so any `?key=`, from every
        // provider error. Mirrors `rpc_hydrate_edition`'s handling.
        Err(e) => {
            tracing::warn!(error = ?e, "hydrate edition failed");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
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
    // Returned as-is. An earlier version backfilled an absent `title` from the
    // free text, which for an author-only search copied the author's name into
    // the title slot — asking Open Library for a book whose *title* contains
    // "Frank Herbert" and scoring every candidate against that, which is the
    // precise bug this whole path exists to remove. A query with no title is
    // handled honestly downstream: `relevance::filter_and_rank` has nothing to
    // rank against and returns the providers' own results unscored.
    structured
}

#[cfg(test)]
mod tests;
