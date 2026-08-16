//! Wire types for ISBN → book-metadata resolution. A scan that misses the
//! library is resolved server-side against external providers (Open Library,
//! then Google Books) into one normalized [`ExternalBookMeta`] the client uses
//! to prefill the check-in / manual-entry screens.

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
    /// Human-readable provider name, for the check-in note, the settings
    /// panel, and the provider catalog — one string, one place, rather than a
    /// `match` re-written at every render site.
    pub fn display_name(self) -> &'static str {
        match self {
            MetadataProvider::OpenLibrary => "Open Library",
            MetadataProvider::GoogleBooks => "Google Books",
            MetadataProvider::Hardcover => "Hardcover",
        }
    }
}

/// What one provider can be asked and what it can return, independent of
/// whether it is currently configured. Lets a caller (the eventual
/// provider-filter UI) skip asking a provider a question it can never
/// answer, and skip rendering a column no provider actually fills.
///
/// `carries_ratings` and `carries_genres` are `false` for every provider
/// today: [`ExternalBookMeta`] has no field for either yet, so claiming
/// either capability would be a promise no provider can keep. Flip a
/// provider's flag to `true` in the same change that adds the field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub search_by_title: bool,
    pub search_by_isbn: bool,
    pub carries_cover: bool,
    pub carries_ratings: bool,
    pub carries_genres: bool,
}

/// One entry in the provider catalog: identity, whether it is usable right
/// now, and what it can answer. Built by `providers::catalog` in `omnibus-db`
/// and served by `GET /api/metadata/providers`.
///
/// **Never carries key material.** `configured` is a bool, not a masked
/// preview — unlike [`GoogleBooksKeyStatus`] and
/// `omnibus_db::HardcoverKeyStatus`, this type has no `masked` field at all,
/// because this endpoint is reachable by any authenticated user, not just an
/// admin.
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

#[cfg(test)]
mod tests;
