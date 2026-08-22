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

use super::providers::{self, openlibrary};
use super::query::SearchQuery;
use super::{MetadataLookupConfig, MetadataLookupError};

/// Re-fetch one selected candidate in full.
///
/// `Ok(None)` when the provider no longer knows the candidate — a clean miss
/// the caller answers by keeping the list row it already has, never by
/// blanking it.
///
/// **The handle, not the ISBN, is what identifies the candidate.** `isbn13`
/// is a hint: when present it names a *printing*, so it is preferred, and the
/// provider is asked by ISBN exactly as before. When absent — which is now
/// routine, since a Hardcover search document describes a work whose `isbns`
/// span every edition — the provider is asked by its own `provider_ref`
/// instead. A malformed ISBN is treated as absent rather than fatal, because
/// the handle alone is enough and refusing the whole re-fetch over a bad hint
/// would strand a candidate the reader can see.
///
/// Google Books and Hardcover already answer their searches from the same
/// record their detail endpoint would serve, so for those two the ISBN path is
/// the ISBN lookup and nothing more.
pub async fn hydrate_edition(
    config: &MetadataLookupConfig,
    source: MetadataProvider,
    provider_ref: &str,
    raw_isbn: Option<&str>,
) -> Result<Option<ProviderEdition>, MetadataLookupError> {
    // Gated like every other caller. `providers::run` only clears a cooldown
    // when the call recorded none of its own, which relies on nothing asking a
    // provider that is already cooling down — and without this, clicking a
    // candidate fired requests at the very source the search had just rendered
    // as "rate limited, skipping for 10m".
    if let Some(remaining) = config.throttle.remaining(source) {
        return Err(MetadataLookupError::Provider(anyhow::anyhow!(
            "{source:?} is rate-limited; retry in {}s",
            super::throttle::retry_after_secs(remaining)
        )));
    }
    let isbn13 = raw_isbn.and_then(|raw| normalize_isbn(raw).ok());

    let detail = match isbn13.as_deref() {
        Some(isbn) => {
            let query = SearchQuery {
                isbn13: Some(isbn.to_string()),
                ..SearchQuery::default()
            };
            providers::run(source, config, &query)
                .await?
                .into_iter()
                .next()
        }
        None => providers::by_ref(source, config, provider_ref).await?,
    };
    let Some(mut detail) = detail else {
        return Ok(None);
    };

    // Only on the ISBN path, and concurrently. `by_isbn` answers from
    // `jscmd=data`, which carries no description, so the record has to be
    // fetched separately — but `by_ref` already GETs that exact
    // `{provider_ref}.json` document and reads the description out of it, so
    // calling `describe` on the handle path re-fetched the same body only to
    // throw the result away.
    if source == MetadataProvider::OpenLibrary {
        if let Some(isbn) = isbn13.as_deref() {
            let (description, enrichment) = tokio::join!(
                openlibrary::describe(config, provider_ref),
                openlibrary::enrich(config, isbn)
            );
            detail.description = detail.description.or(description);
            detail.series = detail.series.or(enrichment.series);
            detail.first_publish_year = detail.first_publish_year.or(enrichment.first_publish_year);
        }
    }

    // The handle the caller selected by, not the one this lookup happened to
    // mint: the picker keys its selection on the value it already holds.
    detail.provider_ref = provider_ref.to_string();
    Ok(Some(detail))
}
