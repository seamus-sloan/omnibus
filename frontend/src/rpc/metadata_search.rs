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
use omnibus_shared::metadata_lookup::{EditionHydrateRequest, EditionSearchRequest};
use omnibus_shared::metadata_lookup::{
    EditionSearchResponse, MetadataProvider, ProviderEdition, ProviderInfo,
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
pub async fn rpc_search_editions(query: String) -> Result<EditionSearchResponse> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    let req = EditionSearchRequest {
        query,
        providers: None,
    };
    req.validate().map_err(ServerFnError::new)?;
    Ok(db::search_all_providers(&provider_config(&pool.0).await?, req.query.trim(), None).await)
}

/// Re-fetch one selected candidate from the provider that offered it.
///
/// `Ok(None)` when that provider no longer knows the ISBN — the caller keeps
/// the candidate it already has rather than blanking the row. Same
/// `can_edit` gate as the search, for the same reason.
#[post("/api/rpc/metadata/editions/hydrate", pool: PoolExt, user: AuthUser)]
pub async fn rpc_hydrate_edition(
    source: MetadataProvider,
    provider_ref: String,
    isbn13: String,
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
        req.isbn13.trim(),
    )
    .await
    // Both variants carry a caller-facing sentence — a bad ISBN names the
    // input, and the provider variant is deliberately worded for a reader
    // rather than exposing the provider's own wording. The chain below it
    // stays on the log.
    .map_err(|e| {
        tracing::warn!("hydrate edition failed: {e:#}");
        ServerFnError::new(e.to_string())
    })?)
}
