//! The metadata editor's edition picker: a fan-out search across every
//! configured provider, and the follow-up detail fetch for whichever
//! candidate the reader selects.
//!
//! The web analogue of the `/api/metadata/*` REST routes mobile would use —
//! same db calls, same gates. Nothing here writes: applying a candidate goes
//! through the edit form's existing `rpc_save_overrides` path.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
#[cfg(feature = "server")]
use omnibus_shared::metadata_lookup::EditionHydrateRequest;
// `EditionSearchRequest` is in the server function's *signature*, so it has to
// resolve on the client half the macro generates too — it cannot be gated.
use omnibus_shared::metadata_lookup::{
    EditionSearchRequest, EditionSearchResponse, MetadataProvider, ProviderEdition, ProviderInfo,
};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// The provider config for this instance: saved settings keys win over the
/// env vars, the same resolution the REST handlers use, so the two can never
/// disagree about which providers are configured.
#[cfg(feature = "server")]
async fn provider_config(
    pool: &sqlx::SqlitePool,
) -> Result<db::MetadataLookupConfig, ServerFnError> {
    Ok(db::MetadataLookupConfig::live(
        db::provider_keys(pool)
            .await
            .map_err(|e| internal_rpc_error("metadata provider keys", e))?,
    ))
}

/// The provider catalog: who exists, who is usable right now, and what each
/// can answer. Readable by any authenticated user — it carries no key
/// material, only `configured: bool` — because the picker has to decide
/// whether to offer a search at all before it knows the reader may edit.
#[get("/api/rpc/metadata/providers", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_metadata_providers() -> Result<Vec<ProviderInfo>> {
    Ok(db::catalog(&provider_config(&pool.0).await?))
}

/// Search every configured provider at once, keeping each source's
/// candidates attributed and un-collapsed.
///
/// `can_edit`-gated like the overrides *write* it feeds, because it is the
/// one metadata read that spends outbound provider calls. Always `Ok` once
/// the query is well-formed: a provider that fails reports `Failed` on its
/// own status row rather than failing the whole search.
#[post("/api/rpc/metadata/editions/search", pool: PoolExt, user: AuthUser)]
pub async fn rpc_search_editions(req: EditionSearchRequest) -> Result<EditionSearchResponse> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    req.validate().map_err(ServerFnError::new)?;
    Ok(db::search_all_providers(
        &provider_config(&pool.0).await?,
        &search_query(&req),
        req.providers.as_deref(),
    )
    .await)
}

/// Turn a request into the query the providers are actually asked.
///
/// The structured fields win when the caller sent them; free text falls back
/// to a title-only query, which is the only honest reading of a string a
/// reader typed — splitting it on a guess would be worse than not splitting
/// it at all.
#[cfg(feature = "server")]
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

/// Re-fetch one selected candidate from the provider that offered it.
///
/// `Ok(None)` when that provider no longer knows the candidate — the caller
/// keeps the one it already has rather than blanking the row. Same `can_edit`
/// gate as the search, for the same reason.
#[post("/api/rpc/metadata/editions/hydrate", pool: PoolExt, user: AuthUser)]
pub async fn rpc_hydrate_edition(
    source: MetadataProvider,
    provider_ref: String,
    isbn13: Option<String>,
) -> Result<Option<ProviderEdition>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    let req = EditionHydrateRequest {
        source,
        provider_ref,
        isbn13,
    };
    req.validate().map_err(ServerFnError::new)?;
    Ok(db::hydrate_edition(
        &provider_config(&pool.0).await?,
        req.source,
        req.provider_ref.trim(),
        req.isbn13
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
    )
    .await
    // Both variants carry a caller-facing sentence — a bad ISBN names the
    // input, and the provider variant is deliberately worded for a reader
    // rather than exposing the provider's own wording.
    //
    // Logged with `?e`, not `{e:#}`: thiserror lowers a literal
    // `#[error("…")]` to a `write_str`, which ignores the alternate flag, so
    // `{e:#}` records the same fixed sentence the caller already got and the
    // provider's own cause reaches nothing. `Debug` walks the chain. Mirrors
    // `rpc::scan`'s handling of this same error type, and is safe to log in
    // full because `providers::http::strip_url` has already removed the
    // request URL — and so any `?key=` — from every provider error.
    .map_err(|e| {
        tracing::warn!(error = ?e, "hydrate edition failed");
        ServerFnError::new(e.to_string())
    })?)
}
