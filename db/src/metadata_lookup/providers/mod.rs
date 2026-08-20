//! The provider clients and the single point that dispatches to them.
//!
//! **Every provider module implements the same pair**, and nothing else:
//!
//! ```ignore
//! pub async fn by_isbn(&MetadataLookupConfig, isbn13: &str)
//!     -> anyhow::Result<Option<ProviderEdition>>;
//! pub async fn by_title(&MetadataLookupConfig, query: &str)
//!     -> anyhow::Result<Vec<ProviderEdition>>;
//! ```
//!
//! A clean miss is an empty answer (`None` / `vec![]`); an `Err` means the
//! provider could not be asked, which is a different thing and is what lets
//! the ladder in [`super`] tell "this book isn't out there" apart from "we
//! never got an answer". Adding a provider is: a module implementing the pair,
//! a [`MetadataProvider`] variant, and an arm in [`run`].

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

use super::MetadataLookupConfig;

#[cfg(test)]
mod tests;

/// The two things a rung can be asked. Carrying the operation as data (rather
/// than passing an async closure) keeps the ladder a single non-generic
/// function — a generic `AsyncFn`-bound helper trips a rustc higher-ranked
/// lifetime inference bug in this crate, the same one documented on
/// `suggestions::hardcover::resolve_book`.
#[derive(Debug, Clone, Copy)]
pub enum Query<'a> {
    /// A canonical ISBN-13.
    Isbn(&'a str),
    /// Free title text as the reader typed it.
    Title(&'a str),
}

/// Ask one provider one question.
///
/// Both operations answer with a `Vec` so callers need only one shape: an
/// ISBN lookup returns at most one entry, and empty means a clean miss either
/// way. This is the **single** dispatch point — the check-in ladder
/// ([`super::climb`]) and the editor's fan-out
/// ([`super::search_all_providers`]) both come through here, so adding a
/// provider stays one arm rather than two.
///
/// [`ProviderEdition`] is the richer shape; the ladder narrows it to
/// `ExternalBookMeta` on its way out.
pub async fn run(
    provider: MetadataProvider,
    config: &MetadataLookupConfig,
    query: Query<'_>,
) -> anyhow::Result<Vec<ProviderEdition>> {
    match (provider, query) {
        (MetadataProvider::OpenLibrary, Query::Isbn(isbn)) => {
            Ok(openlibrary::by_isbn(config, isbn)
                .await?
                .into_iter()
                .collect())
        }
        (MetadataProvider::OpenLibrary, Query::Title(q)) => openlibrary::by_title(config, q).await,
        (MetadataProvider::GoogleBooks, Query::Isbn(isbn)) => {
            Ok(googlebooks::by_isbn(config, isbn)
                .await?
                .into_iter()
                .collect())
        }
        (MetadataProvider::GoogleBooks, Query::Title(q)) => googlebooks::by_title(config, q).await,
        (MetadataProvider::Hardcover, Query::Isbn(isbn)) => Ok(hardcover::by_isbn(config, isbn)
            .await?
            .into_iter()
            .collect()),
        (MetadataProvider::Hardcover, Query::Title(q)) => hardcover::by_title(config, q).await,
    }
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
/// round trip and its title search is exact-match only, so it earns its place
/// answering what the catalogs couldn't rather than slowing down the common
/// case. Being non-terminal, a Hardcover outage can never turn a clean
/// "we couldn't find it" into a user-facing provider error.
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
    vec![
        ProviderInfo {
            id: MetadataProvider::OpenLibrary,
            display_name: MetadataProvider::OpenLibrary.display_name().to_string(),
            configured: true,
            requires_key: false,
            capabilities: COMMON_CAPABILITIES,
        },
        ProviderInfo {
            id: MetadataProvider::GoogleBooks,
            display_name: MetadataProvider::GoogleBooks.display_name().to_string(),
            configured: true,
            requires_key: false,
            capabilities: COMMON_CAPABILITIES,
        },
        ProviderInfo {
            id: MetadataProvider::Hardcover,
            display_name: MetadataProvider::Hardcover.display_name().to_string(),
            configured: config.keys.hardcover.is_some(),
            requires_key: true,
            capabilities: COMMON_CAPABILITIES,
        },
    ]
}
