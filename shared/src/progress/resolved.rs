//! Server-resolved position vocabulary: where a stored position actually
//! sits in the book, and the enriched per-book progress envelope
//! `GET /api/progress/{uuid}` serves. Every derived figure is computed on
//! the read path from the structure tables — nothing here is stored, so no
//! write path can forget to bump it.

use serde::{Deserialize, Serialize};

use crate::cross_format::CrossFormatCandidate;
use crate::progress::{ProgressFormat, ProgressRecord};

/// How much to trust a [`ResolvedPosition`]. Reported rather than implied:
/// an unresolvable position must say so, because a caller that cannot tell
/// "we know" from "we guessed" will present a guess as fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PositionConfidence {
    /// Derived from the position the client actually stored, resolved
    /// against extracted structure: a CFI against spine stats, or audio
    /// seconds against real container chapter marks.
    Exact,
    /// Derived, but through a lossy step — a whole-book percent mapped back
    /// onto the spine, or audio seconds with no chapter marks to name.
    Approximate,
    /// Nothing was derivable. Every field alongside is `None`.
    Unknown,
}

/// Where a stored position sits in the book, in the vocabulary a reader
/// uses. Always present on a progress record so "what chapter am I on?" is
/// answerable without client-side CFI arithmetic; an unmappable position
/// reports [`PositionConfidence::Unknown`] rather than omitting the block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ResolvedPosition {
    /// Spine document the position falls in. `None` for audio rows and for
    /// a position that could not be placed on the spine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spine_index: Option<i64>,
    /// TOC title of the chapter containing the position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_title: Option<String>,
    /// 1-based position of that chapter in the book's chapter list — a
    /// **book** chapter, never an audio part (see [`super::ResumePoint`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_ordinal: Option<i64>,
    /// How many chapters the book has, for a "chapter 7 of 65" readout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapters_total: Option<i64>,
    /// Percent through the containing chapter, 0..=100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_through_chapter: Option<f64>,
    /// Percent through the whole book, 0..=100. Full precision — unlike
    /// `ProgressRecord::progress_percent`, which is a stored integer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_through_book: Option<f64>,
    pub confidence: PositionConfidence,
}

impl ResolvedPosition {
    /// The "nothing was derivable" answer. Returned rather than `None` so
    /// every record carries the block and a caller reads one shape.
    pub fn unknown() -> Self {
        Self {
            spine_index: None,
            chapter_title: None,
            chapter_ordinal: None,
            chapters_total: None,
            percent_through_chapter: None,
            percent_through_book: None,
            confidence: PositionConfidence::Unknown,
        }
    }
}

/// One format's stored position plus everything the server can derive from
/// it. Mirrors [`super::ResumePoint`]'s shape — the record nested under a
/// named field rather than flattened — so the two enriched progress reads
/// read the same way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ProgressDetail {
    pub record: ProgressRecord,
    /// Where the position sits in the book.
    pub resolved: ResolvedPosition,
    /// Whole-book audio duration (sum of parts), so no caller ever needs an
    /// audiobook's runtime out of band. `None` for epub rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration_seconds: Option<f64>,
    /// The user's saved playback rate. `None` for epub rows and when no
    /// preference has been saved (treat as 1x).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_rate: Option<f64>,
    /// 1-based **audio part** at the saved position — a container mark in
    /// the resolved audio file, not a book chapter. See
    /// [`super::ResumePoint::audio_part`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_part: Option<i64>,
    /// How many audio parts that file carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_part_count: Option<i64>,
}

/// Response body of `GET /api/progress/{uuid}`: **every** format the user
/// has a position in for this book, plus which one is furthest.
///
/// The envelope replaced a bare `Option<ProgressRecord>` that defaulted to
/// `format=epub`. That default was a trap: a reader 87% through the
/// audiobook and 47% through the EPUB read back as 47%, with nothing in the
/// payload indicating another format existed, was further along, or was
/// written more recently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BookProgress {
    /// Canonical book uuid the records resolved to — not necessarily the
    /// one asked for, which may be a merged-away alias.
    pub book_uuid: String,
    /// One entry per format the user has a position in, `epub` first.
    /// Empty when the user has never opened the book.
    pub records: Vec<ProgressDetail>,
    /// Which record represents the reader's true place in the book — the
    /// one furthest through it, ties broken by most recent event time. A
    /// caller that reads nothing but this field gets the right answer.
    /// `None` when `records` is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub furthest: Option<ProgressFormat>,
    /// Whether the user has confirmed a cross-format link for this book.
    #[serde(default)]
    pub linked: bool,
    /// For linked books: the mapped "resume in the other format" candidate,
    /// measured from [`Self::furthest`]. Absent when the book is unlinked,
    /// the link is stale, or the position is unmappable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_format: Option<CrossFormatCandidate>,
}

impl BookProgress {
    /// The empty answer for a book the user has never opened.
    pub fn empty(book_uuid: String) -> Self {
        Self {
            book_uuid,
            records: Vec::new(),
            furthest: None,
            linked: false,
            cross_format: None,
        }
    }

    /// The record named by [`Self::furthest`], if any.
    pub fn furthest_record(&self) -> Option<&ProgressDetail> {
        let furthest = self.furthest?;
        self.records.iter().find(|d| d.record.format == furthest)
    }
}

#[cfg(test)]
mod tests;
