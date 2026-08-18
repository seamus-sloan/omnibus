//! Fan-out edition search: every selected provider asked concurrently, results
//! kept attributed and un-collapsed, and a per-source status so a caller can
//! tell "no results" apart from "not configured" and "couldn't reach it".
//!
//! A sibling of the check-in ladder, not a replacement — both dispatch through
//! [`providers::run`].

use futures::future::join_all;
use omnibus_shared::metadata_lookup::{
    EditionSearchResponse, ExternalBookMeta, MetadataProvider, ProviderEdition, ProviderInfo,
    ProviderSearchStatus, ProviderSourceStatus, EDITIONS_PER_PROVIDER,
};

use super::providers::{self, Query};
use super::MetadataLookupConfig;

/// Max provider searches in flight at once. Three providers fit in one chunk
/// today; the bound is what keeps a future catalog from opening a socket per
/// source the moment a reader types.
const FANOUT_CONCURRENCY: usize = 3;

/// Search every configured provider for candidate editions of `query`.
///
/// Never fails as a whole: a provider that errors becomes a
/// [`ProviderSearchStatus::Failed`] row and the others still answer, so the
/// caller can always render a 200. `providers` names the subset to ask;
/// `None` means every provider in the catalog. An unconfigured provider is
/// reported as [`ProviderSearchStatus::NotConfigured`] and never sent a
/// request.
///
/// Results are **not** deduped by ISBN — two sources describing the same
/// printing are two entries, which is precisely what an edition picker exists
/// to show.
pub async fn search_all_providers(
    config: &MetadataLookupConfig,
    query: &str,
    providers: Option<&[MetadataProvider]>,
) -> EditionSearchResponse {
    let targets = selected_providers(config, providers);
    let askable: Vec<MetadataProvider> = targets
        .iter()
        .filter(|info| info.configured)
        .map(|info| info.id)
        .collect();

    let mut answers = Vec::with_capacity(askable.len());
    for chunk in askable.chunks(FANOUT_CONCURRENCY) {
        answers.extend(join_all(chunk.iter().map(|p| ask_one(config, *p, query))).await);
    }

    // `answers` is in `askable` order, which is `targets` order with the
    // unconfigured entries removed — so one `next()` per configured target
    // re-pairs them without a lookup table.
    let mut answers = answers.into_iter();
    let mut buckets: Vec<Vec<ProviderEdition>> = Vec::with_capacity(askable.len());
    let mut sources = Vec::with_capacity(targets.len());
    for info in &targets {
        let status = if !info.configured {
            ProviderSearchStatus::NotConfigured
        } else {
            match answers.next() {
                Some(Ok(found)) => {
                    let bucket = to_editions(found);
                    let count = bucket.len();
                    buckets.push(bucket);
                    ProviderSearchStatus::Answered { count }
                }
                Some(Err(message)) => ProviderSearchStatus::Failed { message },
                None => ProviderSearchStatus::Failed {
                    message: "provider was not asked".to_string(),
                },
            }
        };
        sources.push(ProviderSourceStatus {
            provider: info.id,
            display_name: info.display_name.clone(),
            status,
        });
    }

    EditionSearchResponse {
        editions: interleave(buckets),
        sources,
    }
}

/// The catalog entries to ask, in catalog order. An explicit filter selects
/// from the catalog rather than being trusted as a list, so a repeated or
/// unknown entry can neither duplicate a source nor invent one.
fn selected_providers(
    config: &MetadataLookupConfig,
    requested: Option<&[MetadataProvider]>,
) -> Vec<ProviderInfo> {
    let catalog = providers::catalog(config);
    match requested {
        None => catalog,
        Some(list) => catalog
            .into_iter()
            .filter(|info| list.contains(&info.id))
            .collect(),
    }
}

/// Ask one provider, turning its error into a caller-facing message.
///
/// Unlike the ladder, a failure here is reported rather than swallowed — the
/// whole point of the per-source report — so the message is redacted before it
/// leaves this crate.
async fn ask_one(
    config: &MetadataLookupConfig,
    provider: MetadataProvider,
    query: &str,
) -> Result<Vec<ExternalBookMeta>, String> {
    match providers::run(provider, config, Query::Title(query)).await {
        Ok(found) => Ok(found),
        Err(e) => {
            tracing::warn!("{provider:?} edition search failed: {e:#}");
            Err(redact_keys(config, &format!("{e:#}")))
        }
    }
}

/// Blank any configured API key out of a provider message. The providers
/// already strip request URLs from `reqwest` errors; this is the second line
/// of defence for a message built any other way.
fn redact_keys(config: &MetadataLookupConfig, message: &str) -> String {
    let mut out = message.to_string();
    for key in [
        config.keys.googlebooks.as_deref(),
        config.keys.hardcover.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|k| !k.is_empty())
    {
        out = out.replace(key, "[redacted]");
    }
    out
}

/// Cap one provider's answer and attribute each candidate with the handle it
/// can be re-fetched by — the ISBN-13, which is what every provider's
/// `by_isbn` takes. Opaque to the client either way.
fn to_editions(found: Vec<ExternalBookMeta>) -> Vec<ProviderEdition> {
    found
        .into_iter()
        .take(EDITIONS_PER_PROVIDER)
        .map(|meta| {
            let provider_ref = meta.isbn13.clone();
            ProviderEdition::from_meta(meta, provider_ref)
        })
        .collect()
}

/// Flatten the per-provider buckets round-robin, so a source that answered
/// with ten candidates cannot own the head of the list.
fn interleave(buckets: Vec<Vec<ProviderEdition>>) -> Vec<ProviderEdition> {
    let total: usize = buckets.iter().map(Vec::len).sum();
    let mut queues: Vec<_> = buckets.into_iter().map(Vec::into_iter).collect();
    let mut out = Vec::with_capacity(total);
    let mut progressed = true;
    while progressed {
        progressed = false;
        for queue in queues.iter_mut() {
            if let Some(edition) = queue.next() {
                out.push(edition);
                progressed = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
