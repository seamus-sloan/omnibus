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
