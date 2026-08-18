//! Wire types for ISBN → book-metadata resolution. A scan that misses the
//! library is resolved server-side against external providers (Open Library,
//! then Google Books) into one normalized [`ExternalBookMeta`] the client uses
//! to prefill the check-in / manual-entry screens. The fan-out edition search
//! shares the providers but has its own types, so that contract stays frozen.

use serde::{Deserialize, Serialize};

/// Which external provider a lookup resolved against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataProvider {
    OpenLibrary,
    GoogleBooks,
    /// Only reachable on an instance with a Hardcover API key configured.
    Hardcover,
}

impl MetadataProvider {
    /// Human-readable provider name for display (check-in note, settings panel, provider catalog).
    pub fn display_name(self) -> &'static str {
        match self {
            MetadataProvider::OpenLibrary => "Open Library",
            MetadataProvider::GoogleBooks => "Google Books",
            MetadataProvider::Hardcover => "Hardcover",
        }
    }
}

/// What one provider can be asked and what it can return, independent of
/// whether it is currently configured.
///
/// `carries_ratings`/`carries_genres` are `false` for every provider today —
/// flip one only in the same change that adds the field to [`ExternalBookMeta`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub search_by_title: bool,
    pub search_by_isbn: bool,
    pub carries_cover: bool,
    pub carries_ratings: bool,
    pub carries_genres: bool,
}

/// One entry in the provider catalog, built by `providers::catalog` in
/// `omnibus-db` and served by `GET /api/metadata/providers`.
///
/// **Never carries key material** — `configured` is a bool, not a masked
/// preview, because this endpoint is reachable by any authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: MetadataProvider,
    pub display_name: String,
    /// Whether the ladder would actually invoke this provider right now —
    /// always `true` for a keyless-capable provider, key-gated otherwise.
    /// Mirrors the same check `providers::ladder` uses, so the two can never
    /// disagree about what "configured" means.
    pub configured: bool,
    /// Whether an API key is required to reach this provider at all (as
    /// opposed to optional — present or not, the provider still answers).
    pub requires_key: bool,
    pub capabilities: ProviderCapabilities,
}

/// Maximum byte length of a stored Google Books API key. Google keys are short
/// (~39 chars), so this is a generous guard against an unbounded blob.
pub const GOOGLE_BOOKS_API_KEY_MAX_LEN: usize = 512;

/// Masked status of the server-wide Google Books key for the Settings UI.
/// **Never carries the raw key** — only a short masked preview. Mirrors
/// [`crate::suggestion::HardcoverKeyStatus`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleBooksKeyStatus {
    pub configured: bool,
    /// Short masked preview (e.g. `AIza…9f3a`), or `None` when unset.
    pub masked: Option<String>,
    /// Where the effective key comes from: `"settings"`, `"env"`, or `"none"`.
    pub source: String,
}

/// Normalized book metadata resolved from an external provider. Every field but
/// `isbn13`, `title`, and `source` is best-effort — providers disagree on which
/// they carry. `cover_url` is a provider-hosted image the caller fetches only
/// when a book/wishlist entry is actually created, not at lookup time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalBookMeta {
    /// The normalized ISBN-13 that resolved (always 13 digits, no hyphens).
    pub isbn13: String,
    pub title: String,
    pub authors: Vec<String>,
    /// Publication year or date as the provider reports it (free text).
    pub year: Option<String>,
    pub pages: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Series statement as the provider reports it (free text, e.g.
    /// "The Kingkiller Chronicle (1)"). Best-effort Open Library enrichment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    /// Year the *work* was first published, across all editions — distinct
    /// from `year`, which is this edition's own date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_publish_year: Option<i64>,
    pub source: MetadataProvider,
}

impl ExternalBookMeta {
    /// Maximum title length in Unicode scalar values (chars). Mirrors
    /// `MetadataOverrides::TITLE_MAX_LEN`.
    pub const TITLE_MAX_LEN: usize = 500;
    /// Maximum length in chars for single-line name fields: publisher, year,
    /// and each author name. Mirrors `MetadataOverrides::NAME_MAX_LEN`.
    pub const NAME_MAX_LEN: usize = 250;
    /// Maximum length (in chars) for the `description` field. Mirrors
    /// `MetadataOverrides::DESCRIPTION_MAX_LEN`.
    pub const DESCRIPTION_MAX_LEN: usize = 50_000;
    /// Maximum **byte** length for `cover_url`. Mirrors
    /// `crate::AUTHOR_PHOTO_URL_MAX_LEN`, which is itself a byte cap — unlike
    /// every other field cap on this type, `cover_url` is validated with
    /// `.len()` (bytes), not `.chars().count()`.
    pub const COVER_URL_MAX_LEN: usize = crate::AUTHOR_PHOTO_URL_MAX_LEN;
    /// Maximum number of authors.
    pub(crate) const MAX_AUTHORS: usize = 32;

    /// Validate field lengths. This is untrusted wire input — either resolved
    /// from an external provider or hand-edited by the client before being
    /// submitted back (add-physical-only / wishlist-by-meta) — so every
    /// free-text field gets a cap before it reaches a DB write. Returns `Err`
    /// with a human-readable message on the first field that exceeds its cap.
    ///
    /// Length caps are measured in Unicode scalar values (`chars`), not UTF-8
    /// bytes, matching `MetadataOverrides::validate` — **except** `cover_url`,
    /// whose cap is byte-based (see [`Self::COVER_URL_MAX_LEN`]).
    pub fn validate(&self) -> Result<(), String> {
        if self.title.chars().count() > Self::TITLE_MAX_LEN {
            return Err(format!("title exceeds {} characters", Self::TITLE_MAX_LEN));
        }
        if self.authors.len() > Self::MAX_AUTHORS {
            return Err(format!("too many authors (max {})", Self::MAX_AUTHORS));
        }
        for author in &self.authors {
            if author.chars().count() > Self::NAME_MAX_LEN {
                return Err(format!(
                    "author name exceeds {} characters",
                    Self::NAME_MAX_LEN
                ));
            }
        }
        let check = |name: &str, val: &Option<String>, max: usize| -> Result<(), String> {
            if let Some(v) = val {
                if v.chars().count() > max {
                    return Err(format!("{name} exceeds {max} characters"));
                }
            }
            Ok(())
        };
        check("year", &self.year, Self::NAME_MAX_LEN)?;
        check("publisher", &self.publisher, Self::NAME_MAX_LEN)?;
        check("series", &self.series, Self::NAME_MAX_LEN)?;
        check("description", &self.description, Self::DESCRIPTION_MAX_LEN)?;
        if let Some(ref cover_url) = self.cover_url {
            if cover_url.len() > Self::COVER_URL_MAX_LEN {
                return Err(format!(
                    "cover_url exceeds {} bytes",
                    Self::COVER_URL_MAX_LEN
                ));
            }
        }
        Ok(())
    }
}

/// Maximum candidate editions one provider may contribute to a fan-out
/// search. Per-provider rather than overall so a chatty source cannot crowd
/// the quieter ones out of the picker.
pub const EDITIONS_PER_PROVIDER: usize = 10;

/// Maximum length (in chars) for an edition-search `query`. Mirrors the
/// check-in title search's cap so the two front doors bound provider requests
/// identically.
pub const EDITION_SEARCH_QUERY_MAX_LEN: usize = crate::scan::SEARCH_QUERY_MAX_LEN;

/// Maximum number of entries an explicit `providers` filter may carry — a
/// generous multiple of the catalog's size, so a malformed client can't post
/// an unbounded list.
pub const EDITION_SEARCH_MAX_PROVIDERS: usize = 16;

/// One candidate edition from one provider, kept attributed and
/// un-collapsed for the edition picker.
///
/// Deliberately **not** an extension of [`ExternalBookMeta`]: that type is the
/// check-in wire payload, validated and round-tripped back from the client,
/// and keeping its contract frozen is worth more than the reuse. Use
/// [`From<ProviderEdition>`] where a check-in-shaped value is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEdition {
    /// Which provider answered with this candidate.
    pub source: MetadataProvider,
    /// Opaque handle the server can re-fetch this candidate by. Clients must
    /// treat it as a token to hand back, never parse it.
    pub provider_ref: String,
    pub isbn13: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<String>,
    pub pages: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub series: Option<String>,
    pub first_publish_year: Option<i64>,
}

impl ProviderEdition {
    /// Attribute an already-normalized provider result with the handle it can
    /// be re-fetched by. `source` comes from the meta, so a candidate can
    /// never claim a provider that didn't produce it.
    pub fn from_meta(meta: ExternalBookMeta, provider_ref: String) -> Self {
        Self {
            source: meta.source,
            provider_ref,
            isbn13: meta.isbn13,
            title: meta.title,
            authors: meta.authors,
            year: meta.year,
            pages: meta.pages,
            publisher: meta.publisher,
            description: meta.description,
            cover_url: meta.cover_url,
            series: meta.series,
            first_publish_year: meta.first_publish_year,
        }
    }
}

impl From<ProviderEdition> for ExternalBookMeta {
    fn from(edition: ProviderEdition) -> Self {
        Self {
            isbn13: edition.isbn13,
            title: edition.title,
            authors: edition.authors,
            year: edition.year,
            pages: edition.pages,
            publisher: edition.publisher,
            description: edition.description,
            cover_url: edition.cover_url,
            series: edition.series,
            first_publish_year: edition.first_publish_year,
            source: edition.source,
        }
    }
}

/// What one provider did with a fan-out search.
///
/// The three cases are the point of the response: silently dropping a failed
/// provider makes "we couldn't reach Hardcover" indistinguishable from
/// "Hardcover doesn't have it".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderSearchStatus {
    /// The provider answered; `count` is how many of its candidates survived
    /// the per-provider cap.
    Answered { count: usize },
    /// The provider is not usable on this instance (no key), so it was never
    /// sent a request.
    NotConfigured,
    /// The provider was asked and could not answer. `message` never carries
    /// key material.
    Failed { message: String },
}

/// One provider's line in the per-source report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSourceStatus {
    pub provider: MetadataProvider,
    pub display_name: String,
    #[serde(flatten)]
    pub status: ProviderSearchStatus,
}

/// A fan-out edition search: every configured provider asked, or only those
/// named in `providers`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionSearchRequest {
    pub query: String,
    /// `None` means every configured provider — the filter hook the picker's
    /// provider chips plug into.
    #[serde(default)]
    pub providers: Option<Vec<MetadataProvider>>,
}

impl EditionSearchRequest {
    /// Reject a blank or oversized `query` and an oversized provider filter.
    /// Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("query is required".into());
        }
        if self.query.chars().count() > EDITION_SEARCH_QUERY_MAX_LEN {
            return Err(format!(
                "query exceeds {EDITION_SEARCH_QUERY_MAX_LEN} characters"
            ));
        }
        if let Some(providers) = &self.providers {
            if providers.len() > EDITION_SEARCH_MAX_PROVIDERS {
                return Err(format!(
                    "too many providers (max {EDITION_SEARCH_MAX_PROVIDERS})"
                ));
            }
        }
        Ok(())
    }
}

/// Fan-out search results: every candidate, attributed and un-collapsed, plus
/// what each source did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionSearchResponse {
    pub editions: Vec<ProviderEdition>,
    pub sources: Vec<ProviderSourceStatus>,
}

#[cfg(test)]
mod tests;
