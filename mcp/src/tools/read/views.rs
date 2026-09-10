//! MCP-facing projections of the shared wire types.
//!
//! Two jobs, both about what an agent pays to read an answer. Every stamp is
//! emitted as ISO 8601 beside the raw epoch it came from, so a model never
//! does calendar arithmetic to say when something happened; and the feeds
//! that would otherwise inline a whole [`EbookMetadata`] per entry project a
//! [`BookStub`] instead.
//!
//! These types are one-way — the tools serialize them and nothing parses them
//! back — so they carry `Serialize` and no `Deserialize`.

use schemars::JsonSchema;
use serde::Serialize;

use omnibus_shared::cross_format::CrossFormatCandidate;
use omnibus_shared::{
    BookProgress, Bookmark, Contributor, EbookMetadata, Highlight, HighlightColor, JournalEntry,
    JournalStatus, PhysicalCopy, ProgressFormat, ProgressRecord, ReadStatus, ReadStatusRecord,
    ResolvedPosition, ResumePoint, SessionFormat, SessionLogEntry, SessionLogPage,
};

/// Unix seconds as an ISO 8601 UTC stamp (`2026-09-04T03:25:36Z`).
///
/// Delegates to `omnibus_shared::to_iso8601` rather than carrying a second
/// conversion: the same stamp reaches REST clients through the wire types'
/// own `_iso` siblings, and two implementations of one format is how the two
/// surfaces come to disagree about a date.
///
/// Total over `i64` — every epoch renders, so a stamp paired with a required
/// epoch is always present. The previous `time`-backed version returned
/// `None` outside its own representable range; that was a property of the
/// crate, not a decision, and callers no longer carry a branch for it.
pub fn iso(epoch: i64) -> String {
    omnibus_shared::to_iso8601(epoch)
}

/// [`iso`] over an optional stamp: absence passes through, nothing else can.
fn iso_opt(epoch: Option<i64>) -> Option<String> {
    epoch.map(iso)
}

/// Playback rate rounded to the two decimals the UI actually offers.
///
/// The stored value is a float sum, so a 2.3x preference serializes as
/// `2.3000000000000003` and invites a model to repeat that back verbatim.
fn round_rate(rate: f64) -> f64 {
    (rate * 100.0).round() / 100.0
}

/// A book reduced to what a feed entry needs in order to name it and let the
/// caller decide whether to fetch the rest with `get_book`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BookStub {
    /// The uuid handle every per-book tool takes.
    pub uuid: Option<String>,
    pub title: Option<String>,
    pub creators: Vec<Contributor>,
    pub cover_url: Option<String>,
    /// Lowercase file formats present on disk, e.g. `["epub"]`, `["m4b"]`.
    pub formats: Vec<String>,
    pub series: Option<String>,
    pub series_index: Option<String>,
}

impl From<&EbookMetadata> for BookStub {
    fn from(b: &EbookMetadata) -> Self {
        BookStub {
            uuid: b.unique_identifier.clone(),
            title: b.title.clone(),
            creators: b.creators.clone(),
            cover_url: b.cover_url.clone(),
            formats: b.formats.clone(),
            series: b.series.clone(),
            series_index: b.series_index.clone(),
        }
    }
}

/// Either projection of the book on a resume point, chosen by the caller's
/// `verbosity`. Untagged so the field reads as a book either way.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum BookView {
    Stub(BookStub),
    Full(Box<EbookMetadata>),
}

/// A saved position, with both stamps in both forms.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProgressRecordView {
    pub book_uuid: String,
    pub format: ProgressFormat,
    pub epub_cfi: Option<String>,
    pub audio_position_seconds: Option<f64>,
    pub progress_percent: Option<i64>,
    pub kobo_location: Option<String>,
    pub book_file_id: Option<i64>,
    /// Whole-book audio duration, so a runtime is never guessed.
    pub total_duration_seconds: Option<f64>,
    /// Where the position sits in the book. Passed through rather than
    /// reshaped: the server resolves it, and a projection that dropped it
    /// would put every caller back to decoding a CFI by hand.
    pub resolved: Option<ResolvedPosition>,
    /// Server receipt time, ISO 8601.
    pub updated_at: String,
    /// Server receipt time, unix seconds.
    pub updated_at_epoch: i64,
    /// Event time the most-recent-wins ordering resolves on, ISO 8601.
    pub client_updated_at: String,
    /// The same, unix seconds.
    pub client_updated_at_epoch: i64,
}

impl From<ProgressRecord> for ProgressRecordView {
    fn from(r: ProgressRecord) -> Self {
        ProgressRecordView {
            book_uuid: r.book_uuid,
            format: r.format,
            epub_cfi: r.epub_cfi,
            audio_position_seconds: r.audio_position_seconds,
            progress_percent: r.progress_percent,
            kobo_location: r.kobo_location,
            book_file_id: r.book_file_id,
            total_duration_seconds: r.total_duration_seconds,
            resolved: r.resolved,
            updated_at: iso(r.updated_at),
            updated_at_epoch: r.updated_at,
            client_updated_at: iso(r.client_updated_at),
            client_updated_at_epoch: r.client_updated_at,
        }
    }
}

/// One "pick up where you left off" entry.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ResumePointView {
    pub record: ProgressRecordView,
    pub book: BookView,
    pub linked: bool,
    /// For a linked book, the mapped "resume in the other format" candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_format: Option<CrossFormatCandidate>,
    /// 1-based **audio part** at the saved position — a container mark, not
    /// a book chapter. Named for what it is: a 65-chapter novel stored as a
    /// 4-part M4B reports part 4 of 4, and calling that a chapter tells the
    /// reader they have finished a book they are a quarter through. Book
    /// chapters live on `record.resolved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_part: Option<i64>,
    /// How many parts that file carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_part_count: Option<i64>,
    /// The reader's saved speed for this book; absent means 1x.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_rate: Option<f64>,
}

impl ResumePointView {
    /// Project one resume point, keeping the whole book record only when the
    /// caller asked for it.
    pub fn project(point: ResumePoint, full: bool) -> Self {
        let book = if full {
            BookView::Full(Box::new(point.book))
        } else {
            BookView::Stub(BookStub::from(&point.book))
        };
        ResumePointView {
            record: point.record.into(),
            book,
            linked: point.linked,
            cross_format: point.cross_format,
            audio_part: point.audio_part,
            audio_part_count: point.audio_part_count,
            playback_rate: point.playback_rate.map(round_rate),
        }
    }
}

/// A book's read state for the signed-in reader.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadStatusView {
    pub book_uuid: String,
    pub status: ReadStatus,
    pub updated_at: String,
    pub updated_at_epoch: i64,
    /// When the book most recently became `finished`; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at_epoch: Option<i64>,
}

impl From<ReadStatusRecord> for ReadStatusView {
    fn from(r: ReadStatusRecord) -> Self {
        ReadStatusView {
            book_uuid: r.book_uuid,
            status: r.status,
            updated_at: iso(r.updated_at),
            updated_at_epoch: r.updated_at,
            finished_at: iso_opt(r.finished_at),
            finished_at_epoch: r.finished_at,
        }
    }
}

/// A kept line.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HighlightView {
    pub id: i64,
    pub book_uuid: String,
    pub epub_cfi_range: Option<String>,
    pub color: HighlightColor,
    pub note: Option<String>,
    pub text: Option<String>,
    pub client_id: Option<String>,
    pub created_at: String,
    pub created_at_epoch: i64,
}

impl From<Highlight> for HighlightView {
    fn from(h: Highlight) -> Self {
        HighlightView {
            id: h.id,
            book_uuid: h.book_uuid,
            epub_cfi_range: h.epub_cfi_range,
            color: h.color,
            note: h.note,
            text: h.text,
            client_id: h.client_id,
            created_at: iso(h.created_at),
            created_at_epoch: h.created_at,
        }
    }
}

/// A saved place.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BookmarkView {
    pub id: i64,
    pub book_uuid: String,
    pub position: String,
    pub title: Option<String>,
    pub client_id: Option<String>,
    pub created_at: String,
    pub created_at_epoch: i64,
}

impl From<Bookmark> for BookmarkView {
    fn from(b: Bookmark) -> Self {
        BookmarkView {
            id: b.id,
            book_uuid: b.book_uuid,
            position: b.position,
            title: b.title,
            client_id: b.client_id,
            created_at: iso(b.created_at),
            created_at_epoch: b.created_at,
        }
    }
}

/// One journal entry.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct JournalEntryView {
    pub id: i64,
    pub book_uuid: String,
    pub author_id: i64,
    pub author_name: String,
    pub body_md: String,
    pub body_html: String,
    pub progress: Option<u8>,
    pub status: JournalStatus,
    pub client_id: Option<String>,
    pub created_at: String,
    pub created_at_epoch: i64,
    pub updated_at: String,
    pub updated_at_epoch: i64,
}

impl From<JournalEntry> for JournalEntryView {
    fn from(j: JournalEntry) -> Self {
        JournalEntryView {
            id: j.id,
            book_uuid: j.book_uuid,
            author_id: j.author_id,
            author_name: j.author_name,
            body_md: j.body_md,
            body_html: j.body_html,
            progress: j.progress,
            status: j.status,
            client_id: j.client_id,
            created_at: iso(j.created_at),
            created_at_epoch: j.created_at,
            updated_at: iso(j.updated_at),
            updated_at_epoch: j.updated_at,
        }
    }
}

/// One recorded sitting.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SessionLogEntryView {
    pub book_uuid: String,
    pub title: String,
    /// `reading`, `listening`, or `mixed` — a sitting can span both formats,
    /// which is why this vocabulary is wider than a progress record's.
    pub format: SessionFormat,
    pub started_at: String,
    pub started_at_epoch: i64,
    pub ended_at: String,
    pub ended_at_epoch: i64,
    /// Seconds recorded across the sitting — not `ended_at - started_at`.
    pub seconds: i64,
}

impl From<SessionLogEntry> for SessionLogEntryView {
    fn from(e: SessionLogEntry) -> Self {
        SessionLogEntryView {
            book_uuid: e.book_uuid,
            title: e.title,
            format: e.format,
            started_at: iso(e.started_at),
            started_at_epoch: e.started_at,
            ended_at: iso(e.ended_at),
            ended_at_epoch: e.ended_at,
            seconds: e.seconds,
        }
    }
}

/// One page of the session log.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SessionLogPageView {
    pub entries: Vec<SessionLogEntryView>,
    pub next_before: Option<String>,
}

impl From<SessionLogPage> for SessionLogPageView {
    fn from(p: SessionLogPage) -> Self {
        SessionLogPageView {
            entries: p.entries.into_iter().map(Into::into).collect(),
            next_before: p.next_before,
        }
    }
}

/// One physical copy on the shelf.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PhysicalCopyView {
    pub id: i64,
    pub book_uuid: String,
    pub isbn: Option<String>,
    pub added_by_user_id: Option<i64>,
    pub checked_in_at: String,
    pub checked_in_at_epoch: i64,
    pub note: Option<String>,
}

impl From<PhysicalCopy> for PhysicalCopyView {
    fn from(c: PhysicalCopy) -> Self {
        PhysicalCopyView {
            id: c.id,
            book_uuid: c.book_uuid,
            isbn: c.isbn,
            added_by_user_id: c.added_by_user_id,
            checked_in_at: iso(c.checked_in_at),
            checked_in_at_epoch: c.checked_in_at,
            note: c.note,
        }
    }
}

/// Every position a reader holds in one book, with each record's stamps in
/// both forms.
///
/// Projects the whole envelope rather than a single record. A reader 87%
/// through the audiobook and 47% through the EPUB has one true place, and an
/// answer carrying the EPUB row alone reports the wrong one with nothing to
/// signal it — so `furthest` travels with the records it ranks.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BookProgressView {
    pub book_uuid: String,
    /// One entry per format the reader has a position in.
    pub records: Vec<ProgressRecordView>,
    /// Which record represents the reader's true place. A caller that reads
    /// nothing else gets the right answer.
    pub furthest: Option<ProgressFormat>,
    /// Whether the reader has confirmed a cross-format link for this book.
    pub linked: bool,
    /// For a linked book, the mapped "resume in the other format" candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_format: Option<CrossFormatCandidate>,
}

impl From<BookProgress> for BookProgressView {
    fn from(p: BookProgress) -> Self {
        BookProgressView {
            book_uuid: p.book_uuid,
            records: p.records.into_iter().map(Into::into).collect(),
            furthest: p.furthest,
            linked: p.linked,
            cross_format: p.cross_format,
        }
    }
}
