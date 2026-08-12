//! Cross-format progress sync wire types: the resume candidate served by
//! `GET /api/books/{uuid}/cross-format-resume`. Every answer carries its
//! mapping confidence so clients can present it honestly (≈-labeled for
//! the linear tier), and the endpoint states *why* when there is no
//! candidate rather than answering with silence.

use serde::{Deserialize, Serialize};

use crate::progress::ProgressFormat;

/// How a book's multiple audio files relate — declared by the user when
/// confirming a link, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossFormatLinkMode {
    /// Files play end to end as one audiobook, in ordinal order.
    Sequence,
    /// Each file is a complete recording; only the primary is aligned.
    Narrations,
}

/// The tier that produced a mapped position. Linear is the v1 floor;
/// chapter-anchored lands as a sibling variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingConfidence {
    Linear,
    /// Piecewise interpolation through matched chapter anchors.
    ChapterAnchored,
}

/// Why (or whether) the endpoint has a candidate to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossFormatResumeState {
    /// The user has never confirmed a link for this book — sync is off.
    NotLinked,
    /// The audio file set changed since the link was confirmed; mapping
    /// is paused until re-confirmation.
    LinkStale,
    /// Linked, but the other format holds nothing newer to offer.
    NothingNewer,
    /// A mapped position is available in `candidate`.
    Candidate,
}

/// Response shape of the cross-format resume read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossFormatResume {
    pub state: CrossFormatResumeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<CrossFormatCandidate>,
}

impl CrossFormatResume {
    /// A candidate-less answer in the given state.
    pub fn empty(state: CrossFormatResumeState) -> Self {
        Self {
            state,
            candidate: None,
        }
    }
}

/// One mapped resume position. The audio-half fields are set when
/// `target` is audio; `percent` when the target is the ebook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossFormatCandidate {
    /// The format this candidate resumes (the request's `?target=`).
    pub target: ProgressFormat,
    /// The format the position was mapped from.
    pub source_format: ProgressFormat,
    /// Ordering clock (client event time) of the source row, so clients
    /// can de-duplicate prompts against positions they already know.
    pub source_client_updated_at: i64,
    pub confidence: MappingConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_file_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_position_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<i64>,
}

/// Everything the alignment modal renders for one book, in one read:
/// the stored link (if any) with its staleness, both timelines' raw
/// material, and both current positions. The client does the linear
/// preview arithmetic — it must re-run live as the user changes the
/// declaration, so shipping raw data beats shipping one precomputed map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<AlignmentLink>,
    /// Chapter-anchor match statistics for a linked, non-stale book —
    /// `None` when no trustworthy anchoring exists (the linear notice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_match: Option<AlignmentMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ebook: Option<AlignmentEbook>,
    pub audio_files: Vec<AlignmentAudioFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<AlignmentPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening: Option<AlignmentAudioPosition>,
}

/// How well the two chapter structures matched, for the modal's readout
/// ("12 of 14 chapter names matched").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlignmentMatch {
    pub matched: i64,
    pub ebook_chapters: i64,
    pub confidence: MappingConfidence,
}

/// The stored link as the modal needs it, plus whether the audio set has
/// drifted since confirmation (mapping paused until re-confirmed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentLink {
    pub mode: CrossFormatLinkMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_book_file_id: Option<i64>,
    pub stale: bool,
    pub confirmed_at: i64,
}

/// Text-side lane: total visible chars plus chapter tick positions.
/// Absent when the structure backfill hasn't reached the book — the
/// modal then shows the honest low-confidence (linear estimate) notice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentEbook {
    pub total_chars: i64,
    pub chapters: Vec<AlignmentEbookChapter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentEbookChapter {
    pub title: String,
    /// Whole-book percent where the chapter starts, 0..=100.
    pub percent: f64,
}

/// One audio file segment for the lane, in current ordinal order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentAudioFile {
    pub book_file_id: i64,
    pub label: String,
    pub duration_seconds: f64,
    /// Chapter start offsets within this file, seconds from its start.
    pub chapter_starts: Vec<f64>,
}

/// Current reading position (percent may be absent for a fresh CFI whose
/// derivation hasn't landed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<i64>,
    pub client_updated_at: i64,
}

/// Current listening position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentAudioPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_file_id: Option<i64>,
    pub seconds: f64,
    pub client_updated_at: i64,
}

/// The confirm write: the user's declaration for one book. `audio_order`
/// (when present) persists a re-ordering of the audio files' ordinals —
/// library-wide data, so the server gates it on edit permission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmCrossFormatLink {
    pub book_uuid: String,
    pub mode: CrossFormatLinkMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_book_file_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_order: Option<Vec<i64>>,
}

impl ConfirmCrossFormatLink {
    /// Boundary validation: narrations needs a primary; an order list
    /// must not repeat ids.
    pub fn validate(&self) -> Result<(), String> {
        if self.mode == CrossFormatLinkMode::Narrations && self.primary_book_file_id.is_none() {
            return Err("narrations mode requires a primary narration".into());
        }
        if let Some(order) = &self.audio_order {
            let mut seen = std::collections::HashSet::new();
            if !order.iter().all(|id| seen.insert(id)) {
                return Err("audio order must not repeat files".into());
            }
        }
        Ok(())
    }
}
