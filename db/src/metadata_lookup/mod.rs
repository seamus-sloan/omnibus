//! Server-side book-metadata resolution against the external providers.
//!
//! Three shapes for three questions: the **ladder** ([`search_provider_by_isbn`])
//! answers a check-in scan with the first good match, the **fan-out**
//! ([`search_all_providers`]) keeps every provider's candidates attributed for
//! the editor's picker, and [`fetch_all_ratings`] collects the community score
//! each one publishes about a book.

mod config;
mod providers;
mod ratings;
mod search;

#[cfg(test)]
mod tests;

pub use config::{MetadataLookupConfig, ProviderKeys};
pub use providers::{catalog, openlibrary_enrich, OlEnrichment};
pub use ratings::fetch_all_ratings;
pub use search::{search_all_providers, FANOUT_PROVIDER_LIMIT};

use omnibus_shared::isbn::{normalize_isbn, IsbnError};
use omnibus_shared::metadata_lookup::ExternalBookMeta;
use providers::Query;

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

    let (found, enrichment) = tokio::join!(
        climb(config, Query::Isbn(&isbn13)),
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
    climb(config, Query::Title(query)).await
}

/// Walk the provider ladder until one answers.
///
/// The first non-empty answer wins. A rung that *fails* is skipped rather than
/// aborting the walk, because a provider being down says nothing about whether
/// the book exists — but if the walk ends empty and the terminal rung was one
/// of the failures, we never got a real answer, so that error surfaces instead
/// of a "not found" we can't stand behind.
async fn climb(
    config: &MetadataLookupConfig,
    query: Query<'_>,
) -> Result<Vec<ExternalBookMeta>, MetadataLookupError> {
    let mut terminal_error: Option<anyhow::Error> = None;

    for rung in providers::ladder(config) {
        match providers::run(rung.provider, config, query).await {
            // The ladder hands callers the frozen check-in payload, not the
            // picker's richer candidate.
            Ok(found) if !found.is_empty() => {
                return Ok(dedupe_by_isbn(
                    found.into_iter().map(ExternalBookMeta::from).collect(),
                ))
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
