//! Wire types for the book-content full-text search (`GET
//! /api/search/content`). Produced by the server's content-search handler
//! from `book_content_fts` hits; distinct from the metadata search's
//! `EbookLibrary` shape because a content hit cites a chapter, not a book row.

use serde::{Deserialize, Serialize};

/// One content-search hit: a chapter-level citation plus a match excerpt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ContentSearchHit {
    /// Durable book identity (`books.uuid`).
    pub book_uuid: String,
    /// Zero-based spine position of the chapter the match came from.
    pub spine_index: i64,
    /// The book's display title, for rendering the citation.
    pub title: String,
    /// FTS5 `snippet()` excerpt: matched terms wrapped in `[`…`]`, elided
    /// context marked with `…`.
    pub snippet: String,
}

/// Response body for `GET /api/search/content`, best-ranked hit first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ContentSearchResults {
    pub hits: Vec<ContentSearchHit>,
}
