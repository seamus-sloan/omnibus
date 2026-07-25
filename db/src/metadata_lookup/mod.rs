//! Server-side ISBN → book-metadata resolution for scans that miss the
//! library. The ISBN is validated and folded to a canonical ISBN-13, then
//! resolved against two providers in a key-dependent order (see
//! [`lookup_isbn`]). Both missing is a clean "unresolved" (`Ok(None)`) so the
//! caller can offer a manual-entry form; an invalid ISBN is a typed error the
//! UI can act on.

mod providers;

#[cfg(test)]
mod tests;

pub use providers::MetadataLookupConfig;

use omnibus_shared::isbn::{normalize_isbn, IsbnError};
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};

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

    let [primary, fallback] = if config.googlebooks_api_key.is_some() {
        [MetadataProvider::GoogleBooks, MetadataProvider::OpenLibrary]
    } else {
        [MetadataProvider::OpenLibrary, MetadataProvider::GoogleBooks]
    };

    match run_provider(config, &isbn13, primary).await {
        Ok(Some(meta)) => return Ok(Some(meta)),
        Ok(None) => {}
        // The primary is best-effort: a transport/parse failure falls through
        // to the fallback rather than failing the whole lookup.
        Err(e) => tracing::warn!("{primary:?} lookup failed, trying fallback: {e:#}"),
    }

    Ok(run_provider(config, &isbn13, fallback).await?)
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
