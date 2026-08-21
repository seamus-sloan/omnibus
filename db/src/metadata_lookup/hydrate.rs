//! Hydrate-on-select: once the picker's reader chooses one candidate, ask
//! that provider again for the record behind it.
//!
//! Only worth a round trip because a search hit is not the provider's own
//! record — Open Library's search answers *works* and carries no description
//! at all, while its edition record has the publisher and the printing's page
//! count. The two are merged rather than swapped, so nothing the reader
//! already saw in the list can disappear.

use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{MetadataProvider, ProviderEdition};

use super::providers::{self, openlibrary, Query};
use super::{MetadataLookupConfig, MetadataLookupError};

/// Re-fetch one selected candidate in full.
///
/// `Ok(None)` when the provider no longer knows the ISBN — a clean miss the
/// caller answers by keeping the list row it already has, never by blanking
/// it. `provider_ref` is the handle the search handed out; it is only ever
/// used as an Open Library record path, and only after [`openlibrary::describe`]
/// has satisfied itself the shape is one.
///
/// Google Books and Hardcover already answer their searches from the same
/// record their detail endpoint would serve, so for those two this is the
/// ISBN lookup and nothing more.
pub async fn hydrate_edition(
    config: &MetadataLookupConfig,
    source: MetadataProvider,
    provider_ref: &str,
    raw_isbn: &str,
) -> Result<Option<ProviderEdition>, MetadataLookupError> {
    let isbn13 = normalize_isbn(raw_isbn)?;
    let Some(mut detail) = providers::run(source, config, Query::Isbn(&isbn13))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };

    if source == MetadataProvider::OpenLibrary {
        let (description, enrichment) = tokio::join!(
            openlibrary::describe(config, provider_ref),
            openlibrary::enrich(config, &isbn13)
        );
        detail.description = detail.description.or(description);
        detail.series = detail.series.or(enrichment.series);
        detail.first_publish_year = detail.first_publish_year.or(enrichment.first_publish_year);
    }

    // The handle the caller selected by, not the one this lookup happened to
    // mint: the picker keys its selection on the value it already holds.
    detail.provider_ref = provider_ref.to_string();
    Ok(Some(detail))
}
