//! Wire types for the "Fetch from Hardcover" metadata-edit action: a
//! read-only preview of a book's canonical Hardcover-sourced fields, shown
//! next to the current values so the user can selectively apply them as
//! overrides. Mirrors `suggestion::SuggestionsResponse`'s
//! not-configured/found split.

use serde::{Deserialize, Serialize};

/// Result of a "Fetch from Hardcover" lookup for one book. Internally
/// tagged (mirrors `suggestion::SuggestionsResponse`) so the wire shape is a
/// single flat object — `{"status":"found", "title": ..., ...}` — rather
/// than the externally-tagged `{"Found": {...}}` serde's enum default would
/// produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HardcoverFetchResult {
    /// No Hardcover key configured — the UI hides the action. Kept as its
    /// own variant (rather than folding into `NotFound`) so a caller that
    /// renders it anyway shows an accurate reason.
    NotConfigured,
    /// Looked up on Hardcover and no matching book was found.
    NotFound,
    /// A match was found — the fetched fields, ready for preview.
    Found(FetchedBookMetadata),
}

/// A book's canonical Hardcover-sourced fields, previewed against the
/// current metadata on the edit page. Cover art is deliberately not
/// included: applying an overridden cover image (#983) hasn't landed in
/// this codebase yet, so there is nothing for a fetched cover URL to feed
/// into — see the metadata-edit page's "Fetch from Hardcover" panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchedBookMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    pub isbn13: Option<String>,
}
