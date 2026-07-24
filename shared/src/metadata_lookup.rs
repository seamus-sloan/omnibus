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
