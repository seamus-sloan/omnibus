//! Server-side ISBN → book-metadata resolution for scans that miss the
//! library. The ISBN is validated and folded to a canonical ISBN-13, then
//! resolved against two providers in a key-dependent order (see
//! [`lookup_isbn`]). Both missing is a clean "unresolved" (`Ok(None)`) so the
//! caller can offer a manual-entry form; an invalid ISBN is a typed error the
//! UI can act on.

mod providers;

#[cfg(test)]
mod tests;

pub use providers::{openlibrary_enrich, MetadataLookupConfig, OlEnrichment};

use omnibus_shared::isbn::{normalize_isbn, IsbnError};
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};

/// Maximum candidates a title search returns after dedupe. Small on purpose:
/// the picker is a phone-screen list, and a title that needs more than a
/// handful of candidates needs a better query, not a longer page.
pub const SEARCH_LIMIT: usize = 8;

/// Errors from an ISBN metadata lookup.
#[derive(Debug, thiserror::Error)]
pub enum MetadataLookupError {
    /// The input ISBN failed validation (bad length, chars, or check digit).
    #[error(transparent)]
    Isbn(#[from] IsbnError),
    /// A provider was unreachable or returned an unparseable response. Distinct
    /// from a clean miss, which is `Ok(None)`, not an error.
    ///
    /// The message is deliberately the caller-facing sentence rather than the
    /// provider's own wording: "google books returned an error status" reads as
    /// an Omnibus bug, when in practice it is usually Google Books' anonymous
    /// daily quota (HTTP 429 — we send no API key). The provider's error stays
    /// on the source chain for the log.
    #[error("metadata lookup is temporarily unavailable — try again later, or enter the details manually")]
    Provider(#[from] anyhow::Error),
}

/// Resolve `raw_isbn` to external book metadata.
///
/// Validates and normalizes the ISBN (→ [`MetadataLookupError::Isbn`] on a bad
/// input), then tries two providers in order: the primary is best-effort (a
/// miss *or* an error falls through), and only a failure of the fallback
/// surfaces as [`MetadataLookupError::Provider`]. Returns `Ok(None)` when both
/// cleanly miss — the manual-entry signal — and `Ok(Some(_))` on the first hit.
///
/// **Order depends on the Google Books key.** With a key configured, Google
/// Books leads: keyless it shares an anonymous quota it quickly exhausts (HTTP
/// 429), but a keyed instance gets richer, more reliable data, so it's the
/// better primary. Without a key, Open Library leads and Google Books is the
/// (usually-throttled) fallback — the historical order.
pub async fn lookup_isbn(
    config: &MetadataLookupConfig,
    raw_isbn: &str,
) -> Result<Option<ExternalBookMeta>, MetadataLookupError> {
    let isbn13 = normalize_isbn(raw_isbn)?;

    // Enrichment (series, first-publish year) needs only the ISBN, so it runs
    // concurrently with the provider chain — a hit pays no extra latency for
    // the bonus fields, and a miss just wastes two cheap best-effort GETs.
    let (resolved, enrichment) = tokio::join!(
        lookup_chain(config, &isbn13),
        providers::openlibrary_enrich(config, &isbn13)
    );
    Ok(resolved?.map(|mut meta| {
        meta.series = meta.series.or(enrichment.series);
        meta.first_publish_year = meta.first_publish_year.or(enrichment.first_publish_year);
        meta
    }))
}

/// The key-dependent primary → fallback provider chain behind [`lookup_isbn`].
async fn lookup_chain(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> Result<Option<ExternalBookMeta>, MetadataLookupError> {
    let [primary, fallback] = provider_order(config);

    match run_provider(config, isbn13, primary).await {
        Ok(Some(meta)) => return Ok(Some(meta)),
        Ok(None) => {}
        // The primary is best-effort: a transport/parse failure falls through
        // to the fallback rather than failing the whole lookup.
        Err(e) => tracing::warn!("{primary:?} lookup failed, trying fallback: {e:#}"),
    }

    Ok(run_provider(config, isbn13, fallback).await?)
}

/// Search the providers by title text — the scan flow's fallback when the
/// ISBN itself resolves nothing. Same key-dependent order and semantics as
/// [`lookup_isbn`]: the primary is best-effort (an empty answer *or* an error
/// falls through), and only a failure of the fallback surfaces as
/// [`MetadataLookupError::Provider`]. Candidates are deduped by ISBN-13 and
/// capped at [`SEARCH_LIMIT`]. Callers validate the query shape
/// (`ScanSearchRequest::validate`) before this is reached.
pub async fn search_title(
    config: &MetadataLookupConfig,
    query: &str,
) -> Result<Vec<ExternalBookMeta>, MetadataLookupError> {
    let [primary, fallback] = provider_order(config);

    match run_search(config, query, primary).await {
        Ok(results) if !results.is_empty() => return Ok(dedupe_by_isbn(results)),
        Ok(_) => {}
        Err(e) => tracing::warn!("{primary:?} search failed, trying fallback: {e:#}"),
    }

    Ok(dedupe_by_isbn(run_search(config, query, fallback).await?))
}

/// The key-dependent provider order shared by the ISBN lookup and the title
/// search (see [`lookup_isbn`] for the reasoning).
fn provider_order(config: &MetadataLookupConfig) -> [MetadataProvider; 2] {
    if config.googlebooks_api_key.is_some() {
        [MetadataProvider::GoogleBooks, MetadataProvider::OpenLibrary]
    } else {
        [MetadataProvider::OpenLibrary, MetadataProvider::GoogleBooks]
    }
}

/// Drop repeat editions (Google Books in particular answers the same ISBN
/// more than once) and cap the picker's length, keeping first-seen order.
fn dedupe_by_isbn(results: Vec<ExternalBookMeta>) -> Vec<ExternalBookMeta> {
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter(|m| seen.insert(m.isbn13.clone()))
        .take(SEARCH_LIMIT)
        .collect()
}

/// Dispatch a single provider lookup, so [`lookup_isbn`] can order the two
/// providers by name rather than duplicating the miss/error handling per order.
async fn run_provider(
    config: &MetadataLookupConfig,
    isbn13: &str,
    provider: MetadataProvider,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    match provider {
        MetadataProvider::OpenLibrary => providers::openlibrary_lookup(config, isbn13).await,
        MetadataProvider::GoogleBooks => providers::googlebooks_lookup(config, isbn13).await,
    }
}

/// Dispatch a single provider title search — [`run_provider`]'s twin for
/// [`search_title`].
async fn run_search(
    config: &MetadataLookupConfig,
    query: &str,
    provider: MetadataProvider,
) -> anyhow::Result<Vec<ExternalBookMeta>> {
    match provider {
        MetadataProvider::OpenLibrary => providers::openlibrary_search(config, query).await,
        MetadataProvider::GoogleBooks => providers::googlebooks_search(config, query).await,
    }
}
