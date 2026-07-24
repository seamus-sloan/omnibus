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
    /// Maximum byte length for `cover_url`. Mirrors
    /// `crate::AUTHOR_PHOTO_URL_MAX_LEN`.
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
    /// bytes, matching `MetadataOverrides::validate`.
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
        check("description", &self.description, Self::DESCRIPTION_MAX_LEN)?;
        check("cover_url", &self.cover_url, Self::COVER_URL_MAX_LEN)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
