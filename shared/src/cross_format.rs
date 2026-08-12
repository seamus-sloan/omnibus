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
