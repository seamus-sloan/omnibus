//! Wire types for the library-cleanup dedup-suggestion surface, persisted by
//! the `dedup_suggestions` / `cleanup_log` / `entity_aliases` tables. Free of
//! server-only dependencies so it compiles under both the `web` and
//! `mobile` frontend feature gates.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// What domain entity a dedup suggestion is about. Serializes as the same
/// lowercase/snake_case token stored in `dedup_suggestions.kind` /
/// `entity_aliases.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupKind {
    /// Two or more author records that name the same person.
    Author,
    /// Two or more series records that name the same series.
    Series,
    /// A tag-vocabulary entry (merge two tags, or split one into several).
    Tag,
    /// A book title that normalizes to a form the metadata should adopt.
    BookTitle,
}

impl CleanupKind {
    /// The lowercase token persisted in `dedup_suggestions.kind` and
    /// `entity_aliases.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::Series => "series",
            Self::Tag => "tag",
            Self::BookTitle => "book_title",
        }
    }

    /// Parse a persisted/wire token; `None` if unrecognized.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "author" => Self::Author,
            "series" => Self::Series,
            "tag" => Self::Tag,
            "book_title" => Self::BookTitle,
            _ => return None,
        })
    }
}

/// What a dedup suggestion proposes doing. Serializes as the same
/// lowercase/snake_case token stored in `dedup_suggestions.action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAction {
    /// Combine two or more entities into one (author/series/tag merges).
    Merge,
    /// Break one entity into several (a tag whose members diverge).
    Split,
    /// Adopt a normalized name/title in place (book-title normalization).
    Rename,
    /// Remove an entity outright (e.g. an orphaned tag).
    Delete,
    /// Convert an `ignored_authors` blocklist entry into an
    /// `entity_aliases` mapping onto a canonical entity.
    Alias,
}

impl CleanupAction {
    /// The lowercase token persisted in `dedup_suggestions.action`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Split => "split",
            Self::Rename => "rename",
            Self::Delete => "delete",
            Self::Alias => "alias",
        }
    }

    /// Parse a persisted/wire token; `None` if unrecognized.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "merge" => Self::Merge,
            "split" => Self::Split,
            "rename" => Self::Rename,
            "delete" => Self::Delete,
            "alias" => Self::Alias,
            _ => return None,
        })
    }
}

/// A suggestion's review state, matching the `dedup_suggestions.decision`
/// CHECK constraint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Not yet reviewed — the initial state every detected suggestion starts in.
    #[default]
    Pending,
    /// Reviewed and approved; a follow-up apply step acts on it.
    Accepted,
    /// Reviewed and dismissed; left in place as a record so detection
    /// doesn't re-suggest it (the UNIQUE constraint keys on the payload,
    /// not the decision).
    Rejected,
}

impl Decision {
    /// The lowercase token persisted in `dedup_suggestions.decision`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    /// Parse a persisted/wire token; `None` if unrecognized.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

/// Summary counts for the Library Cleanup dashboard badge — how many
/// suggestions are in each review state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupCounts {
    pub pending: i64,
    pub accepted: i64,
    pub rejected: i64,
}

/// A hydrated preview of one suggestion, for rendering a card in the review
/// UI. `secondary_name` and `photo_url` are optional because they don't
/// universally apply — a tag split has no single "other" name, and only
/// author/series merges have a photo to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionCard {
    pub id: i64,
    pub kind: CleanupKind,
    pub action: CleanupAction,
    pub decision: Decision,
    /// The primary entity's display name (e.g. the merge target, or the
    /// tag being split).
    pub primary_name: String,
    /// The other entity's display name, when the suggestion involves
    /// exactly one (a two-way merge). `None` for splits with several
    /// resulting names, or book-title normalization.
    pub secondary_name: Option<String>,
    /// How many library books are affected if the suggestion is applied.
    pub book_count: i64,
    /// Relative URL to a cached author/series photo, when `kind` has one.
    pub photo_url: Option<String>,
    /// Unix seconds the suggestion was detected.
    pub created_at: i64,
}

/// One `ignored_authors` blocklist entry, for the Settings blocklist
/// management list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredAuthor {
    /// The blocklisted name, exactly as stored.
    pub name: String,
    /// Unix seconds the name was blocklisted.
    pub ignored_at: i64,
}
