//! Fan-out edition search: every configured provider asked at once, answers
//! kept attributed and un-collapsed, and a per-source status so a caller can
//! tell "nothing found" apart from "never asked" and "couldn't reach it".
//!
//! The sibling of [`super::climb`], not a replacement — check-in still wants
//! one answer; the metadata editor's picker wants all of them.

use futures::future::join_all;
use omnibus_shared::metadata_lookup::{
    EditionSearchResponse, MetadataProvider, ProviderEdition, ProviderInfo, ProviderSearchSource,
    ProviderSearchStatus,
};

use super::providers::{self, Query};
use super::MetadataLookupConfig;

/// Max provider calls in flight at once, mirroring the bounded-concurrency
/// shape of `author_photos::cascade::refetch_all`. Above today's catalog size
/// on purpose: it is the ceiling a growing catalog will meet, not a limit that
/// binds on three providers. Shared with the ratings fan-out, which asks the
/// same catalog the same way.
pub(super) const FANOUT_CONCURRENCY: usize = 6;

/// Max candidates one provider may contribute, so a chatty source can't
/// crowd the others out of the picker.
///
/// Distinct from `super::SEARCH_LIMIT`, which sizes the *check-in* picker to a
/// phone screen and also caps what each provider is asked for upstream — so in
/// practice that request limit binds first, and this is the backstop for a
/// provider that answers with more than it was asked for.
pub const FANOUT_PROVIDER_LIMIT: usize = 10;

/// Search every configured provider concurrently and return their candidates
/// side by side.
///
/// `providers` names the subset to ask; `None` means every configured one.
/// Unconfigured providers are reported, never requested.
///
/// Infallible by design: a provider that fails becomes
/// [`ProviderSearchStatus::Failed`] on its own row rather than an error for
/// the whole search, because one source being down says nothing about what the
/// others found.
pub async fn search_all_providers(
    config: &MetadataLookupConfig,
    query: &str,
    providers: Option<&[MetadataProvider]>,
) -> EditionSearchResponse {
    let selected: Vec<ProviderInfo> = providers::catalog(config)
        .into_iter()
        .filter(|p| providers.is_none_or(|want| want.contains(&p.id)))
        .collect();

    // `join_all` answers in input order regardless of completion order, so
    // the status rows below stay in catalog order.
    let mut answers = Vec::with_capacity(selected.len());
    for chunk in selected.chunks(FANOUT_CONCURRENCY) {
        answers.extend(join_all(chunk.iter().map(|info| ask_one(info, config, query))).await);
    }

    let mut editions = Vec::new();
    let mut sources = Vec::with_capacity(answers.len());
    for (provider, answer) in answers {
        let status = match answer {
            Answer::Found(found) => {
                let before = editions.len();
                editions.extend(found.into_iter().take(FANOUT_PROVIDER_LIMIT));
                ProviderSearchStatus::Answered {
                    count: editions.len() - before,
                }
            }
            Answer::NotConfigured => ProviderSearchStatus::NotConfigured,
            Answer::Failed(message) => ProviderSearchStatus::Failed { message },
        };
        sources.push(ProviderSearchSource {
            provider,
            display_name: provider.display_name().to_string(),
            status,
        });
    }
    EditionSearchResponse { editions, sources }
}

/// One provider's contribution, before it is split into candidates and a
/// status row.
enum Answer {
    Found(Vec<ProviderEdition>),
    NotConfigured,
    Failed(String),
}

/// Ask one provider, tagging the answer with who gave it. An unconfigured
/// provider short-circuits here, so it is never sent a request that could only
/// fail — the reason the catalog's `configured` is consulted rather than the
/// provider's own error.
async fn ask_one(
    info: &ProviderInfo,
    config: &MetadataLookupConfig,
    query: &str,
) -> (MetadataProvider, Answer) {
    if !info.configured {
        return (info.id, Answer::NotConfigured);
    }
    match providers::run(info.id, config, Query::Title(query)).await {
        Ok(found) => (info.id, Answer::Found(found)),
        Err(e) => {
            tracing::warn!(provider = ?info.id, "edition search failed: {e:#}");
            // The top-level context only: the chain below it can render a
            // request URL, and Google Books' carries the API key. This string
            // is served to the caller, not just logged.
            (info.id, Answer::Failed(e.to_string()))
        }
    }
}
