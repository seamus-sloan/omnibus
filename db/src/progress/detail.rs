//! Position enrichment shared by the per-book progress read and the resume
//! cards: resolve a stored position into the vocabulary a reader uses
//! (chapter, percent through it, percent through the book), attach the
//! audio runtime and playback rate, and name which format is furthest.
//!
//! Everything here is derived on the read path from the structure tables —
//! `epub_spine_stats` / `ebook_chapters` for reading, `file_chapters` /
//! `book_file_parts` for listening. Nothing is stored, so no write path can
//! forget to bump it.

use omnibus_shared::{
    parse_comic_page_anchor, BookProgress, ChapterInfo, PositionConfidence, ProgressDetail,
    ProgressFormat, ProgressRecord, ResolvedPosition,
};
use sqlx::{Row, SqlitePool};

use crate::epub_structure::{self, EbookChapterRow, SpineStatRow};
use crate::hls;

use super::{state::get_progress, ProgressError};

/// Both formats' enriched positions for one book, plus which is furthest.
///
/// `format` narrows the returned records to that one format; `None` returns
/// every format the user has a position in. The narrowing is a filter over
/// the same envelope, not a different response shape — a caller that always
/// reads `records` works either way.
pub async fn book_progress(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    format: Option<ProgressFormat>,
) -> Result<BookProgress, ProgressError> {
    let Some(canonical) = crate::resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(BookProgress::empty(book_uuid.to_string()));
    };
    let wanted: &[ProgressFormat] = match format {
        Some(ProgressFormat::Epub) => &[ProgressFormat::Epub],
        Some(ProgressFormat::Audio) => &[ProgressFormat::Audio],
        None => &[ProgressFormat::Epub, ProgressFormat::Audio],
    };

    let mut records = Vec::with_capacity(wanted.len());
    for fmt in wanted {
        let Some(record) = get_progress(pool, user_id, &canonical, *fmt).await? else {
            continue;
        };
        records.push(enrich(pool, user_id, record).await?);
    }

    let furthest = furthest_format(&records);
    let mut envelope = BookProgress {
        book_uuid: canonical.clone(),
        records,
        furthest,
        linked: false,
        cross_format: None,
    };
    attach_cross_format(pool, user_id, &canonical, &mut envelope).await?;
    Ok(envelope)
}

/// Lift one stored record into a [`ProgressDetail`]: resolve where it sits,
/// and for audio attach the runtime, rate, and part figures.
pub async fn enrich(
    pool: &SqlitePool,
    user_id: i64,
    mut record: ProgressRecord,
) -> Result<ProgressDetail, ProgressError> {
    match record.format {
        ProgressFormat::Audio => {
            let totals = audio_totals(pool, &record.book_uuid, &record).await?;
            let Some(totals) = totals else {
                // An audio row whose book has no audio file left: drop the
                // stored id rather than hand a CTA one that resolves to
                // nothing, and report the position as unplaceable.
                record.book_file_id = None;
                return Ok(ProgressDetail {
                    record,
                    resolved: ResolvedPosition::unknown(),
                    total_duration_seconds: None,
                    playback_rate: None,
                    audio_part: None,
                    audio_part_count: None,
                });
            };
            // Overwrite rather than trust the stored id: it may name a
            // `book_files` row the reindex has since replaced.
            record.book_file_id = Some(totals.book_file_id);
            let position = record.audio_position_seconds.unwrap_or(0.0);
            let resolved = resolve_audio_position(&totals, position);
            // The one figure a caller cannot compute without the runtime,
            // which is why every audio record carries it: guessing an
            // audiobook's length from training data is how a 21h47m book
            // gets reported as 21h12m.
            record.progress_percent = resolved
                .percent_through_book
                .map(|p| (p.floor() as i64).clamp(0, 100));
            let playback_rate = playback_rate(pool, user_id, &record.book_uuid).await?;
            Ok(ProgressDetail {
                record,
                resolved,
                total_duration_seconds: Some(totals.total_duration_seconds),
                playback_rate,
                audio_part: totals.audio_part,
                audio_part_count: totals.audio_part_count,
            })
        }
        ProgressFormat::Epub => {
            record.book_file_id = None;
            let resolved = resolve_epub_position(pool, &record).await?;
            Ok(ProgressDetail {
                record,
                resolved,
                total_duration_seconds: None,
                playback_rate: None,
                audio_part: None,
                audio_part_count: None,
            })
        }
    }
}

/// The user's saved playback rate for a book, rounded on the way out.
///
/// The stored value is whatever float the client sent, so a rate the user
/// set with two taps serializes as `2.3000000000000003`. Players step in
/// tenths (and hundredths at most), so two decimals is lossless here and
/// spares every caller the noise.
async fn playback_rate(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<f64>, ProgressError> {
    let rate = sqlx::query(
        "SELECT playback_rate FROM audiobook_playback_preferences
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(book_uuid)
    .fetch_optional(pool)
    .await?
    .map(|row| row.try_get::<f64, _>("playback_rate"))
    .transpose()?;
    Ok(rate.map(round2))
}

/// Round to two decimals, the precision an audiobook rate control offers.
pub fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Which record represents the reader's true place: furthest through the
/// book, ties broken by most recent event time.
///
/// Distance beats recency deliberately. "Where am I in this book" is a
/// question about the book, and a reader who listened to 87% and then
/// opened the EPUB at 47% to check a name has not un-read the 87%.
fn furthest_format(records: &[ProgressDetail]) -> Option<ProgressFormat> {
    records
        .iter()
        .max_by(|a, b| {
            let a_pct = a.resolved.percent_through_book.unwrap_or(-1.0);
            let b_pct = b.resolved.percent_through_book.unwrap_or(-1.0);
            a_pct
                .total_cmp(&b_pct)
                .then(a.record.client_updated_at.cmp(&b.record.client_updated_at))
        })
        .map(|d| d.record.format)
}

/// Attach the cross-format link state and, when linked, the mapped
/// "resume in the other format" candidate measured from the furthest
/// record. A book that vanished mid-pass is routine — only real DB errors
/// propagate.
async fn attach_cross_format(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    envelope: &mut BookProgress,
) -> Result<(), ProgressError> {
    let link = crate::cross_format::get_link(pool, user_id, book_uuid)
        .await
        .map_err(super::resume::cross_format_error)?;
    if link.is_none() {
        return Ok(());
    }
    envelope.linked = true;
    let Some(furthest) = envelope.furthest else {
        return Ok(());
    };
    let target = match furthest {
        ProgressFormat::Epub => ProgressFormat::Audio,
        ProgressFormat::Audio => ProgressFormat::Epub,
    };
    match crate::cross_format::resume_candidate(pool, user_id, book_uuid, target).await {
        Ok(resume) => envelope.cross_format = resume.candidate,
        Err(
            crate::cross_format::CrossFormatError::BookNotFound
            | crate::cross_format::CrossFormatError::AudioSetMismatch,
        ) => {}
        Err(e) => return Err(super::resume::cross_format_error(e)),
    }
    Ok(())
}

// --- EPUB -------------------------------------------------------------

/// Resolve a reading position against the stored spine/TOC structure.
///
/// The precise path needs a CFI *and* extracted structure; every failure
/// along it degrades to the stored integer percent rather than erroring,
/// and reports the degradation through `confidence` instead of hiding it.
async fn resolve_epub_position(
    pool: &SqlitePool,
    record: &ProgressRecord,
) -> Result<ResolvedPosition, ProgressError> {
    let stored_percent = record
        .progress_percent
        .filter(|p| (0..=100).contains(p))
        .map(|p| p as f64);

    let Some(book_id) = crate::resolve_book_id_by_uuid(pool, &record.book_uuid).await? else {
        return Ok(percent_only(stored_percent));
    };
    let Some((file_id, epub_path)) = crate::book_file_with_id(pool, book_id, "EPUB").await? else {
        return Ok(percent_only(stored_percent));
    };
    let stats = epub_structure::get_spine_stats(pool, file_id).await?;
    if stats.is_empty() {
        return Ok(percent_only(stored_percent));
    }
    let chapters = epub_structure::get_chapters(pool, file_id).await?;

    // A comic page anchor lives in the `epub_cfi` slot but is not a CFI, so
    // it must never reach the parser — `parse_comic_page_anchor` is what
    // keeps a page index from being misread as a location in the spine.
    let comic = record
        .epub_cfi
        .as_deref()
        .and_then(parse_comic_page_anchor)
        .is_some();
    let anchor = match (&record.epub_cfi, comic) {
        (Some(cfi), false) => {
            let cfi = cfi.clone();
            tokio::task::spawn_blocking(move || {
                crate::kobo_position::cfi_spine_offset(&epub_path, &cfi)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
        }
        _ => None,
    };

    // Exact when a real CFI resolved; otherwise fall back to placing the
    // stored percent on the spine, which is a guess at spine granularity
    // and says so.
    let (spine_index, offset, confidence) = match anchor {
        Some((spine_index, offset)) => (spine_index as i64, offset, PositionConfidence::Exact),
        None => {
            let Some(percent) = stored_percent else {
                return Ok(ResolvedPosition::unknown());
            };
            match epub_structure::position_at_fraction(&stats, percent / 100.0) {
                Some((spine_index, offset)) => {
                    (spine_index, offset, PositionConfidence::Approximate)
                }
                None => return Ok(percent_only(stored_percent)),
            }
        }
    };

    let percent_through_book = epub_structure::fraction_at(&stats, spine_index, offset)
        .map(|f| f * 100.0)
        .or(stored_percent);
    let absolute = absolute_chars(&stats, spine_index, offset);
    let (chapter, chapter_confidence) = chapter_at(&chapters, spine_index, absolute);

    Ok(ResolvedPosition {
        spine_index: Some(spine_index),
        chapter_title: chapter.map(|c| c.title.clone()),
        // 1-based for the readout; `ebook_chapters.ordinal` is 0-based TOC
        // order, so a caller never has to know which convention it got.
        chapter_ordinal: chapter.map(|c| c.ordinal + 1),
        chapters_total: (!chapters.is_empty()).then_some(chapters.len() as i64),
        percent_through_chapter: chapter
            .and_then(|c| percent_through_chapter(&chapters, &stats, c, absolute)),
        percent_through_book,
        // The weaker of the two claims wins: an exact CFI placed inside a
        // spine document holding several chapters still cannot say which.
        confidence: weakest(confidence, chapter_confidence),
    })
}

/// A position we can only express as a whole-book percent.
fn percent_only(percent: Option<f64>) -> ResolvedPosition {
    match percent {
        Some(percent) => ResolvedPosition {
            percent_through_book: Some(percent),
            confidence: PositionConfidence::Approximate,
            ..ResolvedPosition::unknown()
        },
        None => ResolvedPosition::unknown(),
    }
}

/// The lower of two confidences — a chain is only as strong as its weakest
/// link, and reporting the stronger one would overstate what is known.
fn weakest(a: PositionConfidence, b: PositionConfidence) -> PositionConfidence {
    use PositionConfidence::{Approximate, Exact, Unknown};
    match (a, b) {
        (Unknown, _) | (_, Unknown) => Unknown,
        (Approximate, _) | (_, Approximate) => Approximate,
        (Exact, Exact) => Exact,
    }
}

/// Whole-book visible-char offset of a position, saturating throughout so a
/// corrupt offset clamps rather than overflowing.
fn absolute_chars(stats: &[SpineStatRow], spine_index: i64, offset_in_file: u64) -> i64 {
    let before = stats
        .iter()
        .find(|s| s.spine_index == spine_index)
        .map(|s| s.chars_before.max(0))
        .unwrap_or(0);
    before.saturating_add(i64::try_from(offset_in_file).unwrap_or(i64::MAX))
}

/// The chapter containing a position, plus how well it is known.
///
/// `ebook_chapters.start_chars` is recorded at **spine granularity** — every
/// TOC entry pointing into one spine document shares that document's start —
/// so a spine document holding several chapters cannot say which of them the
/// reader is in. That case names the chapter the document opens with and
/// reports [`PositionConfidence::Approximate`]: it is the only one the data
/// supports, and it cannot overstate progress.
fn chapter_at(
    chapters: &[EbookChapterRow],
    spine_index: i64,
    absolute: i64,
) -> (Option<&EbookChapterRow>, PositionConfidence) {
    if chapters.is_empty() {
        return (None, PositionConfidence::Approximate);
    }
    let sharing_spine = chapters
        .iter()
        .filter(|c| c.spine_index == spine_index)
        .count();
    let found = chapters
        .iter()
        .filter(|c| c.start_chars <= absolute)
        // A position before every TOC entry (front matter) matches nothing
        // here, which is the intent: name no chapter rather than the first.
        .max_by_key(|c| (c.start_chars, std::cmp::Reverse(c.ordinal)));
    let confidence = if sharing_spine > 1 {
        PositionConfidence::Approximate
    } else {
        PositionConfidence::Exact
    };
    (found, confidence)
}

/// Percent through the containing chapter. `None` when the chapter has no
/// measurable extent — the tied-start case above, where every chapter in one
/// spine document begins at the same offset and a percent would be invented.
fn percent_through_chapter(
    chapters: &[EbookChapterRow],
    stats: &[SpineStatRow],
    chapter: &EbookChapterRow,
    absolute: i64,
) -> Option<f64> {
    // A sibling starting at the same offset means this chapter's extent is
    // not recorded at all: the spine document holds several TOC entries and
    // they all inherit its start. Measuring to the *next distinct* start
    // would hand this chapter the whole document.
    if chapters
        .iter()
        .any(|c| c.ordinal != chapter.ordinal && c.start_chars == chapter.start_chars)
    {
        return None;
    }
    let total: i64 = stats
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
    let end = chapters
        .iter()
        .filter(|c| c.start_chars > chapter.start_chars)
        .map(|c| c.start_chars)
        .min()
        .unwrap_or(total);
    let width = end - chapter.start_chars;
    if width <= 0 {
        return None;
    }
    let into = (absolute - chapter.start_chars).clamp(0, width);
    Some((into as f64 / width as f64 * 100.0).clamp(0.0, 100.0))
}

// --- Audio ------------------------------------------------------------

/// Which audio file a position plays, its runtime, and the container marks
/// measured against that file.
pub(super) struct AudioTotals {
    pub book_file_id: i64,
    pub total_duration_seconds: f64,
    pub audio_part: Option<i64>,
    pub audio_part_count: Option<i64>,
    chapters: Vec<ChapterInfo>,
    /// Whether `chapters` is the sync layer's per-part fallback rather than
    /// marks the container actually carried.
    synthetic_chapters: bool,
}

/// Resolve the audio file for a progress row and measure duration + marks
/// against it. `None` when the book has no resolvable audio file.
///
/// The row's stored `book_file_id` picks the file for a book carrying more
/// than one audiobook, so the readout names the narration the user was
/// actually in. It is a soft reference (rule 06) — a stale id, or one
/// belonging to another book, falls back to the first audio file by ordinal.
pub(super) async fn audio_totals(
    pool: &SqlitePool,
    uuid: &str,
    record: &ProgressRecord,
) -> Result<Option<AudioTotals>, ProgressError> {
    let stored = match record.book_file_id {
        Some(id) => hls::resolve_audiobook_file(pool, uuid, Some(id)).await?,
        None => None,
    };
    let resolved = match stored {
        Some(resolved) => resolved,
        None => match hls::resolve_audiobook(pool, uuid).await? {
            Some(resolved) => resolved,
            None => return Ok(None),
        },
    };
    let parts = hls::get_parts(pool, resolved.book_file_id).await?;
    let total: f64 = parts.iter().map(|p| p.duration_seconds).sum();
    let mut chapters = hls::get_chapters(pool, resolved.book_file_id).await?;
    chapters.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
    let position = record.audio_position_seconds.unwrap_or(0.0);
    Ok(Some(AudioTotals {
        book_file_id: resolved.book_file_id,
        total_duration_seconds: total,
        audio_part: mark_number_at(&chapters, position),
        audio_part_count: (!chapters.is_empty()).then_some(chapters.len() as i64),
        synthetic_chapters: is_synthetic(&chapters, parts.len()),
        chapters,
    }))
}

/// Whether a file's chapter rows are the sync layer's synthetic fallback —
/// one `"Part N"` row per part, written when the container carried no marks
/// of its own (`sync::insert_chapters`).
///
/// It matters because those rows are not chapters. A 65-chapter book stored
/// as a 4-part M4B has four of them, and naming that "chapter 4 of 4" tells
/// a reader they are at the end of the book. Detected by matching the
/// fallback's own output exactly, so a container that genuinely names its
/// chapters "Part 1"… is only misread when it also has one per part — in
/// which case the two are indistinguishable and the cautious reading is the
/// right one.
fn is_synthetic(chapters: &[ChapterInfo], part_count: usize) -> bool {
    !chapters.is_empty()
        && chapters.len() == part_count
        && chapters.iter().enumerate().all(|(i, c)| {
            c.title
                .strip_prefix("Part ")
                .and_then(|n| n.parse::<usize>().ok())
                == Some(i + 1)
        })
}

/// Resolve a listening position against the file's container marks.
///
/// Chapter vocabulary is used only when the marks are real; a synthetic
/// per-part fallback reports the percent and nothing else, because there is
/// no chapter to name (see [`is_synthetic`]).
pub(super) fn resolve_audio_position(totals: &AudioTotals, position: f64) -> ResolvedPosition {
    let percent_through_book = (totals.total_duration_seconds > 0.0)
        .then(|| (position / totals.total_duration_seconds * 100.0).clamp(0.0, 100.0));

    if totals.synthetic_chapters || totals.chapters.is_empty() {
        return ResolvedPosition {
            percent_through_book,
            confidence: PositionConfidence::Approximate,
            ..ResolvedPosition::unknown()
        };
    }

    let index = mark_index_at(&totals.chapters, position);
    let chapter = index.and_then(|i| totals.chapters.get(i));
    ResolvedPosition {
        spine_index: None,
        chapter_title: chapter.map(|c| c.title.clone()),
        chapter_ordinal: index.map(|i| i as i64 + 1),
        chapters_total: Some(totals.chapters.len() as i64),
        percent_through_chapter: chapter.and_then(|c| {
            (c.duration_seconds > 0.0).then(|| {
                ((position - c.start_seconds) / c.duration_seconds * 100.0).clamp(0.0, 100.0)
            })
        }),
        percent_through_book,
        confidence: PositionConfidence::Exact,
    }
}

/// 0-based index of the container mark covering `elapsed`.
fn mark_index_at(chapters: &[ChapterInfo], elapsed: f64) -> Option<usize> {
    if chapters.is_empty() {
        return None;
    }
    Some(
        chapters
            .partition_point(|c| c.start_seconds <= elapsed)
            .saturating_sub(1),
    )
}

/// 1-based container mark number at `elapsed`, mirroring the player's
/// index-plus-one display (not the stored `file_chapters.ordinal`, which is
/// container-supplied and not guaranteed dense).
///
/// `pub(super)` rather than private: exercised directly by a boundary test
/// in `progress::tests` alongside the rest of the resume-card coverage.
pub(super) fn mark_number_at(chapters: &[ChapterInfo], elapsed: f64) -> Option<i64> {
    mark_index_at(chapters, elapsed).map(|i| i as i64 + 1)
}

#[cfg(test)]
mod tests;
