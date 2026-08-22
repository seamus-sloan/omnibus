//! The provider clients and the single point that dispatches to them. Every
//! provider module implements `by_isbn` / `by_title` and nothing else: a clean
//! miss is an empty answer (`None` / `vec![]`), while an `Err` means the
//! provider could not be asked — the distinction [`super`]'s ladder relies on.
//! Adding one is a module, a [`MetadataProvider`] variant, and an arm in [`run`].

// `pub(super)` rather than private so the sibling test module can drive one
// provider in isolation — the bare-text fallback, the key-leak guard, and the
// per-provider parse failures are all provider-level behaviour that the
// ladder's own tests would only reach indirectly.
pub(super) mod googlebooks;
pub(super) mod hardcover;
mod http;
pub(super) mod openlibrary;

/// Re-exported for the sibling test module, which pins the year-trimming rule
/// directly; the providers reach it through `http` instead.
#[cfg(test)]
pub(super) use http::publication_year;
pub use openlibrary::{enrich as openlibrary_enrich, OlEnrichment};

use omnibus_shared::metadata_lookup::{
    MetadataProvider, ProviderCapabilities, ProviderEdition, ProviderInfo,
};

use super::query::SearchQuery;
use super::{relevance, MetadataLookupConfig};

#[cfg(test)]
mod tests;

/// Ask one provider one question, in that provider's own terms.
///
/// Every operation answers with a `Vec` so callers need only one shape: an
/// ISBN lookup returns at most one entry, and empty means a clean miss either
/// way. This is the **single** dispatch point — the check-in ladder
/// ([`super::climb`]) and the editor's fan-out
/// ([`super::search_all_providers`]) both come through here, so adding a
/// provider stays one arm rather than two.
///
/// A query carrying an ISBN uses each provider's exact-identifier lookup,
/// which is the strongest signal any of them accepts; otherwise the title and
/// author go out in whatever form that provider actually understands — a
/// fielded pair to Open Library, bare text to the two full-text engines.
///
/// [`ProviderEdition`] is the richer shape; the ladder narrows it to
/// `ExternalBookMeta` on its way out.
pub async fn run(
    provider: MetadataProvider,
    config: &MetadataLookupConfig,
    query: &SearchQuery,
) -> anyhow::Result<Vec<ProviderEdition>> {
    let found = dispatch(provider, config, query).await;
    match &found {
        // Cleared only when this call recorded no refusal of its own. A
        // provider client can swallow its own failure and still answer `Ok`:
        // `googlebooks::by_isbn` degrades a refused bare-text fallback to a
        // clean miss, so a 429 can be written while the call returns
        // `Ok(None)`. A blanket clear there would erase the refusal we had
        // just learned about. Callers gate before asking, so anything present
        // afterwards is necessarily fresh.
        Ok(_) if config.throttle.remaining(provider).is_none() => config.throttle.clear(provider),
        Ok(_) => {}
        Err(e) => note_throttle(config, provider, e),
    }
    found
}

/// The per-provider arm. Split from [`run`] so the throttle bookkeeping wraps
/// every provider without being repeated in every arm.
async fn dispatch(
    provider: MetadataProvider,
    config: &MetadataLookupConfig,
    query: &SearchQuery,
) -> anyhow::Result<Vec<ProviderEdition>> {
    match (provider, query.isbn13.as_deref()) {
        (MetadataProvider::OpenLibrary, Some(isbn)) => Ok(openlibrary::by_isbn(config, isbn)
            .await?
            .into_iter()
            .collect()),
        (MetadataProvider::OpenLibrary, None) => openlibrary::search(config, query).await,
        (MetadataProvider::GoogleBooks, Some(isbn)) => Ok(googlebooks::by_isbn(config, isbn)
            .await?
            .into_iter()
            .collect()),
        (MetadataProvider::GoogleBooks, None) => googlebooks::search(config, query).await,
        (MetadataProvider::Hardcover, Some(isbn)) => Ok(hardcover::by_isbn(config, isbn)
            .await?
            .into_iter()
            .collect()),
        (MetadataProvider::Hardcover, None) => hardcover::by_text(config, &query.as_text()).await,
    }
}

/// Re-fetch one candidate by the handle its search handed out.
///
/// The counterpart to [`run`]'s ISBN path, for candidates that have no ISBN —
/// which the `provider_ref` was always the real identifier for anyway. Each
/// provider validates the handle's shape before addressing it: it arrives as a
/// claim from the client, not a fact.
pub async fn by_ref(
    provider: MetadataProvider,
    config: &MetadataLookupConfig,
    provider_ref: &str,
) -> anyhow::Result<Option<ProviderEdition>> {
    let found = match provider {
        MetadataProvider::OpenLibrary => openlibrary::by_ref(config, provider_ref).await,
        MetadataProvider::GoogleBooks => googlebooks::by_ref(config, provider_ref).await,
        MetadataProvider::Hardcover => hardcover::by_ref(config, provider_ref).await,
    };
    // Records a refusal but deliberately does **not** clear on success: every
    // provider here answers `Ok(None)` for a handle it won't address, without
    // sending anything — and a request that was never made is no evidence that
    // a cooling-down provider has let us back in. Clearing is [`run`]'s job,
    // which always does reach the network.
    if let Err(e) = &found {
        note_throttle(config, provider, e);
    }
    found
}

/// Record a cooldown when a provider failure was a 429.
///
/// Read off the error chain rather than the response, so Open Library and
/// Hardcover need no per-call-site bookkeeping — both surface the underlying
/// `reqwest::Error` through `.context(...)` and `#[error(transparent)]`
/// respectively. Google Books does not (its retry loop consumes the status and
/// bails with a plain string), which is why it records at its own call site.
///
/// No `Retry-After` is available here — a `reqwest::Error` carries the status
/// but not the headers — so these fall back to the schedule, which is the same
/// thing a provider that sends no such header would get anyway.
fn note_throttle(config: &MetadataLookupConfig, provider: MetadataProvider, error: &anyhow::Error) {
    let throttled = error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<reqwest::Error>())
        .any(|e| e.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS));
    if throttled {
        config.throttle.record(provider, None);
    }
}

/// Ask one provider, retrying with a widened query when a narrow one finds
/// nothing.
///
/// Three rungs, each dropping the most specific term that could have been
/// what excluded an answer:
///
/// 1. the query as given;
/// 2. without the ISBN — a provider that doesn't hold *this printing* may
///    still hold the work;
/// 3. without the author — providers disagree on how much of a name to carry,
///    and one that files "Herbert, Frank" against a query for "Frank Herbert"
///    would otherwise report a clean miss.
///
/// Each rung's results are scored against **that rung's** query, so a
/// title-only retry is not judged for missing the author it was not given.
pub async fn run_with_fallbacks(
    provider: MetadataProvider,
    config: &MetadataLookupConfig,
    query: &SearchQuery,
    limit: usize,
) -> anyhow::Result<Vec<ProviderEdition>> {
    let mut rungs = vec![query.clone()];
    if query.isbn13.is_some() && (query.title.is_some() || query.author.is_some()) {
        rungs.push(query.without_isbn());
    }
    if query.title.is_some() && query.author.is_some() {
        rungs.push(query.without_author());
    }

    let mut found = Vec::new();
    for rung in &rungs {
        // Re-checked before *every* rung, not once by the caller. A rung can
        // record a cooldown and still answer `Ok` — `googlebooks::by_isbn`
        // swallows a refused fallback — and the next rung would then walk
        // straight back into the same 429, escalating the cooldown it just
        // caused inside a single search.
        if config.throttle.remaining(provider).is_some() {
            break;
        }
        found = relevance::filter_and_rank(run(provider, config, rung).await?, rung, limit);
        if !found.is_empty() {
            break;
        }
    }
    Ok(found)
}

/// One rung of the ladder.
pub struct Rung {
    pub provider: MetadataProvider,
    /// Whether this rung's failure is allowed to fail the whole lookup. Set on
    /// exactly one rung — see [`ladder`].
    pub terminal: bool,
}

/// The rungs to try, in order, for `config`.
///
/// **The two catalogs come first**, in a key-dependent order: with a Google
/// Books key configured Google leads, since keyless it shares an anonymous
/// quota it quickly exhausts (HTTP 429) but a keyed instance gets richer, more
/// reliable data. Without a key Open Library leads and Google Books is the
/// (usually-throttled) fallback — the historical order.
///
/// **Hardcover is appended last, and only when keyed.** It costs an extra
/// round trip, so it earns its place answering what the catalogs couldn't
/// rather than slowing down the common case. Being non-terminal, a Hardcover
/// outage can never turn a clean "we couldn't find it" into a user-facing
/// provider error.
///
/// The **terminal** rung is the last catalog: if nothing hit and that rung
/// failed, we never actually got an answer, and saying "not found" would be a
/// lie. Every other rung is best-effort.
pub fn ladder(config: &MetadataLookupConfig) -> Vec<Rung> {
    let [primary, fallback] = if config.keys.googlebooks.is_some() {
        [MetadataProvider::GoogleBooks, MetadataProvider::OpenLibrary]
    } else {
        [MetadataProvider::OpenLibrary, MetadataProvider::GoogleBooks]
    };
    let mut rungs = vec![
        Rung {
            provider: primary,
            terminal: false,
        },
        Rung {
            provider: fallback,
            terminal: true,
        },
    ];
    if config.keys.hardcover.is_some() {
        rungs.push(Rung {
            provider: MetadataProvider::Hardcover,
            terminal: false,
        });
    }
    rungs
}

/// What all three providers can do today: both searches, a cover image, and a
/// genre list — Google Books' `categories`, Open Library's `subjects`, and
/// Hardcover's `cached_tags`. Ratings are nobody's yet.
///
/// One shared constant only holds while the catalog agrees; the moment a
/// provider differs, give it its own value rather than widening this one.
const COMMON_CAPABILITIES: ProviderCapabilities = ProviderCapabilities {
    search_by_title: true,
    search_by_isbn: true,
    carries_cover: true,
    carries_ratings: false,
    carries_genres: true,
};

/// The hosts one provider serves its cover images from.
///
/// Part of the catalog, not a detail of whoever fetches a cover: the picker
/// renders these URLs in the page, so the `img-src` CSP has to name them, and
/// applying one means the server fetching it, which will need the same list
/// as an allowlist. Two copies of that list would drift, and the failure mode
/// is silent — a cover that renders but can't be applied, or the reverse.
///
/// The redirect targets are in it for the same reason the origins are, and
/// they are the surprising part. `covers.openlibrary.org` 302s to
/// `archive.org`, which 302s again to whichever Internet Archive node holds
/// the file (`ia800505.us.archive.org`) — and browsers apply `img-src` to
/// every hop's response, not just the request. The node names rotate, so
/// that last hop can only be expressed as a wildcard.
///
/// An entry beginning `*.` matches any subdomain of the rest, the same
/// meaning CSP gives it.
pub fn cover_hosts(provider: MetadataProvider) -> &'static [&'static str] {
    match provider {
        MetadataProvider::OpenLibrary => {
            &["covers.openlibrary.org", "archive.org", "*.archive.org"]
        }
        MetadataProvider::GoogleBooks => &["books.google.com", "books.googleusercontent.com"],
        MetadataProvider::Hardcover => &["assets.hardcover.app"],
    }
}

/// Every cover host in the catalog, deduplicated and in catalog order.
///
/// Not filtered by `configured`: an unkeyed instance still renders whatever
/// a keyed one wrote, and a CSP that changed shape when a key was saved
/// would be a debugging trap.
pub fn all_cover_hosts() -> Vec<&'static str> {
    let mut hosts: Vec<&'static str> = Vec::new();
    for provider in MetadataProvider::ALL.iter().copied() {
        for host in cover_hosts(provider) {
            if !hosts.contains(host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

/// The full provider catalog: identity, usability, and capabilities for
/// every provider this instance knows about — display surface for the
/// eventual provider-filter UI, and the one place a caller can ask "which
/// sources exist" without matching on [`MetadataProvider`] itself.
///
/// `configured` reuses the exact key-presence check [`ladder`] uses for each
/// provider, so the two can never disagree about what "configured" means:
/// Open Library and Google Books are always usable (Google Books is tried
/// keyless too, just not as the ladder's primary rung — see [`ladder`]'s
/// docs), and Hardcover only when `config.keys.hardcover` is set.
pub fn catalog(config: &MetadataLookupConfig) -> Vec<ProviderInfo> {
    MetadataProvider::ALL
        .iter()
        .copied()
        .map(|id| {
            // The roster comes from `ALL`; whether a provider needs a key, and
            // whether it has one, stays an exhaustive match so a new variant
            // has to answer both rather than inheriting someone else's answer.
            let (configured, requires_key) = match id {
                MetadataProvider::OpenLibrary | MetadataProvider::GoogleBooks => (true, false),
                MetadataProvider::Hardcover => (config.keys.hardcover.is_some(), true),
            };
            ProviderInfo {
                id,
                display_name: id.display_name().to_string(),
                configured,
                requires_key,
                capabilities: COMMON_CAPABILITIES,
            }
        })
        .collect()
}
