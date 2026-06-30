//! Wire types for F3.3 "Readers also enjoyed" suggestions.

use serde::{Deserialize, Serialize};

/// Base URL for outbound Hardcover book links (new tab).
const HARDCOVER_BOOK_BASE: &str = "https://hardcover.app/books/";

/// One suggested read-alike from Hardcover. External to the library — clicking
/// opens `hardcover_url` in a new tab.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookSuggestion {
    pub hardcover_id: i64,
    /// Outbound Hardcover book page.
    pub hardcover_url: String,
    pub title: String,
    pub author: String,
    /// Co-listing count — how many curated Hardcover lists co-shelve this book
    /// with the source book. The relevance signal shown as "on N lists".
    pub list_count: i64,
    /// Relative URL to the cached cover (`/api/suggestions/:uuid/:rank/cover`),
    /// or `None` when no cover was cached (the UI falls back to a typographic
    /// plate). Clients combine it with their configured server base.
    pub cover_url: Option<String>,
}

impl BookSuggestion {
    /// Build a wire suggestion from the cached primitives, deriving the
    /// outbound Hardcover URL (slug-based, id fallback) and the relative cover
    /// URL (only when a cover was cached).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        book_uuid: &str,
        rank: i64,
        hardcover_id: i64,
        hardcover_slug: Option<String>,
        title: String,
        author: String,
        list_count: i64,
        has_cover: bool,
    ) -> Self {
        let hardcover_url = match hardcover_slug {
            Some(slug) if !slug.is_empty() => format!("{HARDCOVER_BOOK_BASE}{slug}"),
            _ => format!("{HARDCOVER_BOOK_BASE}{hardcover_id}"),
        };
        let cover_url = has_cover.then(|| format!("/api/suggestions/{book_uuid}/{rank}/cover"));
        Self {
            hardcover_id,
            hardcover_url,
            title,
            author,
            list_count,
            cover_url,
        }
    }
}

/// Response for a book's suggestions strip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SuggestionsResponse {
    /// No Hardcover key is configured — the UI shows a connect message.
    NotConfigured,
    /// A resolution is enqueued and nothing is cached yet — the UI polls.
    Pending,
    /// Cached results. `items` may be empty (sticky negative — Hardcover
    /// matched nothing usable).
    Ready { items: Vec<BookSuggestion> },
}

/// Masked status of the server-wide Hardcover key for the Settings UI. **Never
/// carries the raw key** — only a short masked preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HardcoverKeyStatus {
    pub configured: bool,
    /// Short masked preview (e.g. `hc_l…3f9a`), or `None` when unset.
    pub masked: Option<String>,
    /// Where the effective key comes from: `"settings"`, `"env"`, or `"none"`.
    pub source: String,
}
