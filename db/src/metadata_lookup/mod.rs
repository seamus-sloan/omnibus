//! Server-side book-metadata resolution against the external providers. Two
//! shapes for two questions: the **ladder** ([`search_provider_by_isbn`] /
//! [`search_provider_by_title`]) walks providers in order and returns the first
//! good answer — right for a check-in scan; the **fan-out**
//! ([`search_all_providers`]) asks every configured provider at once, keeping
//! answers attributed and un-collapsed — right for the editor's edition picker.

mod config;
mod cover;
mod hydrate;
mod providers;
pub mod query;
mod relevance;
mod search;
pub mod throttle;

#[cfg(test)]
mod tests;

pub use config::{MetadataLookupConfig, ProviderKeys};
pub use cover::{provider_cover_image_config, MAX_COVER_REDIRECTS};
pub use hydrate::hydrate_edition;
pub use providers::{all_cover_hosts, catalog, cover_hosts, openlibrary_enrich, OlEnrichment};
pub use query::SearchQuery;
pub use relevance::{filter_and_rank, score_candidate};
pub use search::{search_all_providers, FANOUT_PROVIDER_LIMIT};
pub use throttle::ThrottleTracker;

use omnibus_shared::isbn::{normalize_isbn, IsbnError};
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};

/// Maximum candidates a title search returns after dedupe. Small on purpose:
/// the picker is a phone-screen list, and a title that needs more than a
/// handful of candidates needs a better query, not a longer page.
pub const SEARCH_LIMIT: usize = 8;

/// Errors from a provider search.
#[derive(Debug, thiserror::Error)]
pub enum MetadataLookupError {
    /// The input ISBN failed validation (bad length, chars, or check digit).
    #[error(transparent)]
    Isbn(#[from] IsbnError),
    /// No provider could be asked. Distinct from a clean miss, which is an
    /// empty answer, not an error.
    ///
    /// The message is deliberately the caller-facing sentence rather than the
    /// provider's own wording: "google books returned an error status" reads as
    /// an Omnibus bug, when in practice it is usually Google Books' anonymous
    /// daily quota (HTTP 429 — we send no API key). The provider's error stays
    /// on the source chain for the log.
    #[error("metadata lookup is temporarily unavailable — try again later, or enter the details manually")]
    Provider(#[from] anyhow::Error),
}

/// Resolve an ISBN to book metadata, asking each configured provider in turn.
///
/// Validates and normalizes the ISBN first (→ [`MetadataLookupError::Isbn`] on
/// a bad input), so no provider is ever handed an unvalidated string. Returns
/// `Ok(None)` when the providers answered and none knew the book — the
/// unresolved signal that sends the UI to manual entry or a title search.
///
/// Series and first-publish year are filled in concurrently from Open Library
/// (see [`openlibrary_enrich`]) for whichever fields the answering provider
/// left empty — a hit therefore pays no extra latency for them, and a miss
/// only wastes two cheap best-effort GETs.
pub async fn search_provider_by_isbn(
    config: &MetadataLookupConfig,
    raw_isbn: &str,
) -> Result<Option<ExternalBookMeta>, MetadataLookupError> {
    let isbn13 = normalize_isbn(raw_isbn)?;

    let query = SearchQuery {
        isbn13: Some(isbn13.clone()),
        ..SearchQuery::default()
    };
    let (found, enrichment) = tokio::join!(
        climb(config, &query),
        providers::openlibrary_enrich(config, &isbn13)
    );
    Ok(found?.into_iter().next().map(|mut meta| {
        meta.series = meta.series.or(enrichment.series);
        meta.first_publish_year = meta.first_publish_year.or(enrichment.first_publish_year);
        meta
    }))
}

/// Search the providers by title text — the scan flow's fallback when the ISBN
/// itself resolved nothing. Same ladder and semantics as
/// [`search_provider_by_isbn`]; `Ok(vec![])` when the providers answered and
/// none matched. Callers validate the query shape
/// (`ScanSearchRequest::validate`) before this is reached.
pub async fn search_provider_by_title(
    config: &MetadataLookupConfig,
    query: &str,
) -> Result<Vec<ExternalBookMeta>, MetadataLookupError> {
    climb(config, &SearchQuery::from_text(query)).await
}

/// Walk the provider ladder until one answers.
///
/// The first non-empty answer wins. A rung that *fails* is skipped rather than
/// aborting the walk, because a provider being down says nothing about whether
/// the book exists — but if the walk ends empty and the terminal rung was one
/// of the failures, we never got a real answer, so that error surfaces instead
/// of a "not found" we can't stand behind.
///
/// A rung in a rate-limit cooldown is skipped **without a request**, and
/// counts as a failure for the terminal rule above: we did not get an answer,
/// so "not found" would be just as much of a lie as it is after a 500.
///
/// Each rung's answer is scored before it is accepted. That matters more than
/// it used to: Hardcover's title search was an exact-match filter that could
/// only return the book you named, and is now full-text, so without a floor
/// this walk would take whatever a fuzzy engine ranked first and hand it back
/// as the one confident answer a reader files a physical copy against.
async fn climb(
    config: &MetadataLookupConfig,
    query: &SearchQuery,
) -> Result<Vec<ExternalBookMeta>, MetadataLookupError> {
    let mut terminal_error: Option<anyhow::Error> = None;

    for rung in providers::ladder(config) {
        if let Some(remaining) = config.throttle.remaining(rung.provider) {
            let secs = throttle::retry_after_secs(remaining);
            tracing::warn!(
                "{:?} is rate-limited for another {secs}s, skipping rung",
                rung.provider
            );
            if rung.terminal {
                terminal_error = Some(anyhow::anyhow!(
                    "{:?} is rate-limited; retry in {secs}s",
                    rung.provider
                ));
            }
            continue;
        }
        match providers::run(rung.provider, config, query)
            .await
            .map(|found| relevance::filter_and_rank(found, query, SEARCH_LIMIT))
        {
            // The ladder hands callers the frozen check-in payload, not the
            // picker's richer candidate — and a candidate with no ISBN is not
            // something check-in can act on, so it is dropped here rather than
            // being given a blank one. `filter_map` over `try_into` is what
            // keeps `ExternalBookMeta`'s contract (and the iOS decoder that
            // depends on it) frozen while the picker's loosens.
            Ok(found) if !found.is_empty() => {
                let narrowed = narrow_for_check_in(config, rung.provider, found).await;
                // A rung whose every candidate lacks an ISBN has nothing
                // check-in can use, which is the same as a miss: keep walking.
                if !narrowed.is_empty() {
                    return Ok(dedupe_by_isbn(narrowed));
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("{:?} lookup failed, trying next: {e:#}", rung.provider);
                if rung.terminal {
                    terminal_error = Some(e);
                }
            }
        }
    }

    match terminal_error {
        Some(e) => Err(MetadataLookupError::Provider(e)),
        None => Ok(Vec::new()),
    }
}

/// Narrow a rung's candidates to the check-in payload, resolving an edition
/// for the top one when the whole rung answered at the level of *works*.
///
/// The picker is happy with a work-level candidate — it shows the row and
/// resolves the edition when the reader picks it. Check-in is not: it stores
/// the ISBN per physical copy and prints it on its confirm screens, so a
/// candidate without one is not something it can act on.
///
/// Hardcover's full-text search is exactly this case: its documents describe
/// works, whose `isbns` span every edition, so none can honestly be attributed
/// to the candidate. Without this the rung would answer and then be discarded
/// wholesale — worse than the exact-title filter it replaced, which at least
/// answered with an edition on the rare occasions it matched at all.
///
/// One extra round trip, for the **first** candidate only, and only on a rung
/// that would otherwise contribute nothing. It is bounded on purpose: the
/// check-in flow is a reader standing at a shelf with a scanner, and walking a
/// whole result page one lookup at a time is not worth the wait.
async fn narrow_for_check_in(
    config: &MetadataLookupConfig,
    provider: MetadataProvider,
    found: Vec<omnibus_shared::metadata_lookup::ProviderEdition>,
) -> Vec<ExternalBookMeta> {
    let handle = found.first().map(|e| e.provider_ref.clone());
    let narrowed: Vec<ExternalBookMeta> = found
        .into_iter()
        .filter_map(|e| ExternalBookMeta::try_from(e).ok())
        .collect();
    if !narrowed.is_empty() {
        return narrowed;
    }
    let Some(handle) = handle else {
        return narrowed;
    };
    match providers::by_ref(provider, config, &handle).await {
        Ok(Some(detail)) => ExternalBookMeta::try_from(detail)
            .map(|meta| vec![meta])
            .unwrap_or_default(),
        Ok(None) => Vec::new(),
        Err(e) => {
            // Best-effort: this is a *recovery* from a rung that already had
            // nothing usable, so its failure is that same miss, not a new one.
            tracing::warn!("{provider:?} edition resolve failed: {e:#}");
            Vec::new()
        }
    }
}

/// Drop repeat editions (Google Books in particular answers the same ISBN
/// more than once) and cap the picker's length, keeping first-seen order. A
/// no-op on the ISBN path, which answers with at most one entry.
fn dedupe_by_isbn(results: Vec<ExternalBookMeta>) -> Vec<ExternalBookMeta> {
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter(|m| seen.insert(m.isbn13.clone()))
        .take(SEARCH_LIMIT)
        .collect()
}
