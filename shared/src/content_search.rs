//! Wire types for the book-content full-text search (`GET
//! /api/search/content`). Produced by the server's content-search handler
//! from `book_content_fts` hits; distinct from the metadata search's
//! `EbookLibrary` shape because a content hit cites a chapter, not a book row.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// What to do about hits that sit past the reader's own position.
///
/// The default annotates rather than excludes: an agent that can see a hit
/// is ahead of the reader can say "there's an answer, but it's ahead of
/// you", which is more useful than silence — while `Exclude` is there for
/// the case where the caller must not see the payoff at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpoilerFilter {
    /// Return every hit unmarked.
    None,
    /// Return every hit, each tagged with whether it is ahead of the reader.
    #[default]
    Annotate,
    /// Drop hits past the reader's position and report how many were held
    /// back, so the caller knows an answer exists without seeing it.
    Exclude,
}

/// One content-search hit: a chapter-level citation plus a match excerpt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ContentSearchHit {
    /// Durable book identity (`books.uuid`).
    pub book_uuid: String,
    /// Zero-based spine position of the chapter the match came from.
    pub spine_index: i64,
    /// The book's display title, for rendering the citation.
    pub title: String,
    /// TOC title of the chapter the match came from, when the book's
    /// structure has been extracted. Saves every caller repeating the same
    /// spine-index-to-chapter join by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_title: Option<String>,
    /// FTS5 `snippet()` excerpt: matched terms wrapped in `[`…`]`, elided
    /// context marked with `…`.
    pub snippet: String,
    /// Whether this hit sits past the reader's furthest recorded position in
    /// its book. `None` when the reader has no position there, or when the
    /// book's structure is not extracted enough to place the hit — which is
    /// **not** the same as "safe", and a caller must not render it as such.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead_of_reader: Option<bool>,
    /// How far past (positive) or behind (negative) the reader this hit is,
    /// in whole-book percentage points. `None` on the same terms as
    /// [`Self::ahead_of_reader`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_delta_percent: Option<f64>,
}

/// Response body for `GET /api/search/content`, best-ranked hit first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ContentSearchResults {
    pub hits: Vec<ContentSearchHit>,
    /// How many hits `SpoilerFilter::Exclude` held back. Always present
    /// under that mode (`0` when nothing was withheld) so a caller can say
    /// "there is an answer ahead of you" honestly; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withheld_ahead: Option<i64>,
    /// Why an empty result set is empty, when the query form is the likely
    /// cause. The index matches all terms, so a natural-language phrase
    /// usually matches nothing while its individual words match plenty —
    /// and without this the caller has no reason to suspect the query
    /// rather than the corpus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}
