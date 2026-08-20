//! Community-rating fan-out: every configured provider asked for the score it
//! publishes about one ISBN, concurrently and attributed.
//!
//! Best-effort throughout. A provider that fails, isn't configured, or simply
//! has no rating for the book contributes nothing — all three mean the same
//! thing downstream (no row for that source), which is why this answers with a
//! plain list rather than the per-source status [`super::search_all_providers`]
//! needs.

use futures::future::join_all;
use omnibus_shared::external_ratings::ProviderRating;
use omnibus_shared::metadata_lookup::MetadataProvider;

use super::providers;
use super::search::FANOUT_CONCURRENCY;
use super::MetadataLookupConfig;

/// Ask every configured provider what the internet thinks of `isbn13`.
///
/// Returns one entry per provider that reported a real score, in catalog
/// order. Callers store these; a provider absent from the list is one that
/// has nothing to say, and must not be stored as a zero.
pub async fn fetch_all_ratings(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> Vec<(MetadataProvider, ProviderRating)> {
    let configured: Vec<MetadataProvider> = providers::catalog(config)
        .into_iter()
        .filter(|p| p.configured && p.capabilities.carries_ratings)
        .map(|p| p.id)
        .collect();

    let mut found = Vec::new();
    // `join_all` answers in input order, so the output stays in catalog order.
    for chunk in configured.chunks(FANOUT_CONCURRENCY) {
        let answers = join_all(chunk.iter().map(|id| ask_one(*id, config, isbn13))).await;
        found.extend(answers.into_iter().flatten());
    }
    found
}

/// Ask one provider, degrading a failure to "nothing to report". A rating is
/// an optional adornment on a book — losing one to a provider hiccup costs a
/// line on the detail page, never the write the caller is making.
async fn ask_one(
    provider: MetadataProvider,
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> Option<(MetadataProvider, ProviderRating)> {
    match providers::ratings(provider, config, isbn13).await {
        Ok(rating) => rating.map(|r| (provider, r)),
        Err(e) => {
            tracing::warn!(provider = ?provider, "community rating lookup failed: {e:#}");
            None
        }
    }
}
