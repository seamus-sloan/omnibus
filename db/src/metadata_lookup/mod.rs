//! Server-side ISBN → book-metadata resolution for scans that miss the
//! library. The ISBN is validated and folded to a canonical ISBN-13, then
//! resolved against Open Library first and Google Books as a fallback. Both
//! missing is a clean "unresolved" (`Ok(None)`) so the caller can offer a
//! manual-entry form; an invalid ISBN is a typed error the UI can act on.

mod isbn;
mod providers;

#[cfg(test)]
mod tests;

pub use isbn::{normalize_isbn, IsbnError};
pub use providers::{fetch_cover, MetadataLookupConfig};

use omnibus_shared::metadata_lookup::ExternalBookMeta;

/// Errors from an ISBN metadata lookup.
#[derive(Debug, thiserror::Error)]
pub enum MetadataLookupError {
    /// The input ISBN failed validation (bad length, chars, or check digit).
    #[error(transparent)]
    Isbn(#[from] IsbnError),
    /// A provider was unreachable or returned an unparseable response. Distinct
    /// from a clean miss, which is `Ok(None)`, not an error.
    #[error(transparent)]
    Provider(#[from] anyhow::Error),
}

/// Resolve `raw_isbn` to external book metadata.
///
/// Validates and normalizes the ISBN (→ [`MetadataLookupError::Isbn`] on a bad
/// input), then tries Open Library and, on a miss *or* an error, Google Books.
/// Returns `Ok(None)` when both providers cleanly miss — the manual-entry
/// signal — and `Ok(Some(_))` on the first hit. Only a failure of the fallback
/// provider surfaces as [`MetadataLookupError::Provider`].
pub async fn lookup_isbn(
    config: &MetadataLookupConfig,
    raw_isbn: &str,
) -> Result<Option<ExternalBookMeta>, MetadataLookupError> {
    let isbn13 = normalize_isbn(raw_isbn)?;

    match providers::openlibrary_lookup(config, &isbn13).await {
        Ok(Some(meta)) => return Ok(Some(meta)),
        Ok(None) => {}
        // Open Library is best-effort: a transport/parse failure falls through
        // to the fallback rather than failing the whole lookup.
        Err(e) => tracing::warn!("open library lookup failed, trying google books: {e:#}"),
    }

    Ok(providers::googlebooks_lookup(config, &isbn13).await?)
}
