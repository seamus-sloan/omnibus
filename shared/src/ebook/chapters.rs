//! Wire types for the chapter listing and plain-text chapter reads
//! (`GET /api/ebooks/{uuid}/chapters` and
//! `GET /api/ebooks/{uuid}/chapters/{spine_index}/text`).

use serde::{Deserialize, Serialize};

/// Per-request cap on served chapter text, in characters. A single spine
/// document in a reference work can run to megabytes, so the text read
/// slices at this bound and reports the boundary (`truncated` /
/// `next_offset`) rather than returning an unbounded body. Char-based so a
/// slice can never split a UTF-8 sequence.
pub const CHAPTER_TEXT_MAX_CHARS: usize = 100_000;

/// One chapter in the listing: the TOC title plus the spine index the text
/// read is addressed by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChapterListEntry {
    /// TOC order, 0-based.
    pub ordinal: i64,
    pub title: String,
    /// Index into the spine — the path parameter of the text read.
    pub spine_index: i64,
}

/// `GET /api/ebooks/{uuid}/chapters` response. `has_text: false` is the
/// structured "this book's served format has no extractable text" answer
/// (comic-only, audiobook-only) — distinct from the 404 an unknown uuid
/// gets. A TOC-less but readable EPUB reports `has_text: true` with empty
/// `chapters`; its text is still addressable by spine index up to
/// `spine_count`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChapterListResponse {
    pub book_uuid: String,
    pub has_text: bool,
    /// Number of spine documents; valid text-read indexes are
    /// `0..spine_count`. Zero when `has_text` is false.
    pub spine_count: i64,
    pub chapters: Vec<ChapterListEntry>,
}

/// `GET /api/ebooks/{uuid}/chapters/{spine_index}/text` response: one
/// bounded slice of a spine document's prose. `truncated` marks a slice
/// that ends before the document does, and `next_offset` is the char
/// offset to request next; both are the explicit boundary
/// [`CHAPTER_TEXT_MAX_CHARS`] promises. `has_text: false` mirrors the
/// listing's no-text answer (with everything else zeroed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ChapterTextResponse {
    pub book_uuid: String,
    pub has_text: bool,
    pub spine_index: i64,
    pub text: String,
    /// Char offset this slice starts at (the request's `?offset=`).
    pub offset: i64,
    /// Total chars in the whole document's extracted text.
    pub total_chars: i64,
    pub truncated: bool,
    pub next_offset: Option<i64>,
}
