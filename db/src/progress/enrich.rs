//! Turning a stored position into a place in the book: the audio
//! duration/part totals the resume feed already needed, and the `resolved`
//! chapter block every progress read now carries. Shared by
//! [`super::resume`] and [`super::state`] so the two cannot drift.

use omnibus_shared::{
    parse_comic_page_anchor, ChapterInfo, PositionConfidence, ProgressFormat, ProgressRecord,
    ResolvedPosition,
};
use sqlx::SqlitePool;

use crate::epub_structure::{EbookChapterRow, SpineStatRow};
use crate::hls;

use super::ProgressError;

/// How hard to work to place a reading position.
///
/// The split exists because the two callers ask at very different rates. A CFI
/// names its spine document in the string itself, so the chapter is free; only
/// the offset *within* that document needs the archive opened and one spine
/// document parsed. Paying that per card on every landing load — for a readout
/// that reports a chapter, not a percentage of one — is not a trade worth
/// making, so the feed asks for [`Self::Fast`] and the deliberate single-book
/// read asks for [`Self::Full`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PositionDetail {
    /// Stored structure only: no file I/O. Names the chapter exactly; reports
    /// the whole-book percent the row already carries and no
    /// `percent_through_chapter`.
    Fast,
    /// Walks the CFI for its in-document offset, which is what makes
    /// `percent_through_chapter` and a derived whole-book percent possible.
    Full,
}

/// Which audio file a position plays, plus the duration and structural-part
/// position measured against **that** file.
pub(super) struct AudioTotals {
    pub book_file_id: i64,
    pub total_duration_seconds: f64,
    pub audio_part: Option<i64>,
    pub audio_part_count: Option<i64>,
    /// The file's marks, ascending by start — read once here and reused by
    /// [`resolve_audio`] rather than queried a second time per record.
    marks: Vec<ChapterInfo>,
}

/// Fill a record's read-path fields in place: the resolved audio file and its
/// duration, a whole-book percent for audio rows (which never store one — the
/// write path rejects it), and the `resolved` chapter block.
///
/// Returns the audio totals so a caller that also renders a part readout
/// (the resume feed) doesn't repeat the work. `None` for epub rows and for an
/// audio row whose book has no audio file left.
pub(super) async fn enrich_record(
    pool: &SqlitePool,
    record: &mut ProgressRecord,
    detail: PositionDetail,
) -> Result<Option<AudioTotals>, ProgressError> {
    let audio = match record.format {
        ProgressFormat::Audio => audio_totals(pool, &record.book_uuid, record).await?,
        ProgressFormat::Epub => None,
    };
    match &audio {
        Some(totals) => {
            // Overwrite rather than trust the stored id: it may name a
            // `book_files` row the reindex has since replaced, and the
            // Continue CTA links straight at `?file_id=` — a dead id
            // would open the player on a manifest that 404s.
            record.book_file_id = Some(totals.book_file_id);
            record.total_duration_seconds = Some(totals.total_duration_seconds);
            record.progress_percent = audio_percent(record, totals.total_duration_seconds);
        }
        None => {
            // Audio row whose book has no audio file left (and every epub
            // row): drop the stored id rather than hand a CTA an id that
            // resolves to nothing.
            if matches!(record.format, ProgressFormat::Audio) {
                record.book_file_id = None;
            }
            record.total_duration_seconds = None;
        }
    }
    record.resolved = resolve_position(pool, record, audio.as_ref(), detail).await?;
    Ok(audio)
}

/// Whole-book percent for an audio position, floor semantics and clamping
/// identical to `epub_structure::percent_at` so the two formats report on one
/// ruler. `None` for a book whose parts measured no duration.
fn audio_percent(record: &ProgressRecord, total_duration_seconds: f64) -> Option<i64> {
    // `is_finite` first: a NaN duration would pass a bare `<= 0.0` and
    // produce a NaN percent.
    if !total_duration_seconds.is_finite() || total_duration_seconds <= 0.0 {
        return None;
    }
    let pos = record.audio_position_seconds.unwrap_or(0.0);
    let frac = (pos / total_duration_seconds).clamp(0.0, 1.0);
    Some(((100.0 * frac).floor() as i64).clamp(0, 100))
}

/// Resolve the row's position into a chapter, or `None` when the book carries
/// no structure to resolve against.
///
/// Every underivable case degrades to a coarser block or to `None` — never to
/// a wrong one — mirroring `state::derive_epub_percent`. Only genuine DB
/// errors propagate: an unreadable EPUB or an unparseable CFI is a fact about
/// one book, not a reason to fail the read.
async fn resolve_position(
    pool: &SqlitePool,
    record: &ProgressRecord,
    audio: Option<&AudioTotals>,
    detail: PositionDetail,
) -> Result<Option<ResolvedPosition>, ProgressError> {
    match record.format {
        // Audio marks are already in the database; there is nothing to walk,
        // so the detail level doesn't apply.
        ProgressFormat::Audio => Ok(audio.and_then(|totals| resolve_audio(record, totals))),
        ProgressFormat::Epub => resolve_epub(pool, record, detail).await,
    }
}

/// Map a listening position onto the resolved file's chapter marks.
///
/// The marks are container-supplied and may be the indexer's synthetic
/// per-part fallback ("Part 1", "Part 2", …) — real boundaries, but not
/// chapters. Those report `Low` rather than being withheld, so a caller can
/// still say roughly where the reader is without mistaking four parts for
/// four chapters.
fn resolve_audio(record: &ProgressRecord, totals: &AudioTotals) -> Option<ResolvedPosition> {
    let chapters = &totals.marks;
    let position = record.audio_position_seconds.unwrap_or(0.0);
    let percent_through_book = audio_percent(record, totals.total_duration_seconds);
    let Some(index) = mark_index_at(chapters, position) else {
        // A duration but no marks at all: the percent is still the honest
        // answer to "how far in", so report that much.
        return percent_through_book.map(|percent| ResolvedPosition {
            spine_index: None,
            chapter_title: None,
            chapter_ordinal: None,
            chapters_total: None,
            percent_through_chapter: None,
            percent_through_book: Some(percent),
            confidence: PositionConfidence::Low,
        });
    };
    let mark = &chapters[index];
    let synthetic = chapters
        .iter()
        .all(|c| crate::cross_format::is_synthetic_title(&c.title));
    Some(ResolvedPosition {
        spine_index: None,
        chapter_title: Some(mark.title.clone()),
        chapter_ordinal: Some(index as i64 + 1),
        chapters_total: Some(chapters.len() as i64),
        percent_through_chapter: span_percent(position - mark.start_seconds, mark.duration_seconds),
        percent_through_book,
        confidence: if synthetic {
            PositionConfidence::Low
        } else {
            PositionConfidence::High
        },
    })
}

/// Map a reading position onto the book's spine and table of contents.
///
/// Three position shapes arrive here and each resolves differently: a CFI
/// (which names its own spine document, and yields an in-document offset once
/// walked), a comic page anchor (no spine to speak of — the stored percent is
/// the whole answer), and a percent with no CFI at all, which a Kobo writes.
/// The last is mapped *back* onto the spine, which is necessarily coarse and
/// says so.
async fn resolve_epub(
    pool: &SqlitePool,
    record: &ProgressRecord,
    detail: PositionDetail,
) -> Result<Option<ResolvedPosition>, ProgressError> {
    if let Some(cfi) = record.epub_cfi.as_deref() {
        if parse_comic_page_anchor(cfi).is_some() {
            return Ok(record.progress_percent.map(|percent| ResolvedPosition {
                spine_index: None,
                chapter_title: None,
                chapter_ordinal: None,
                chapters_total: None,
                percent_through_chapter: None,
                percent_through_book: Some(percent),
                confidence: PositionConfidence::High,
            }));
        }
    }
    let Some(book_id) = crate::resolve_book_id_by_uuid(pool, &record.book_uuid).await? else {
        return Ok(None);
    };
    let Some((file_id, source)) = crate::book_file_with_id(pool, book_id, "EPUB").await? else {
        return Ok(None);
    };
    let stats = crate::epub_structure::get_spine_stats(pool, file_id)
        .await
        .map_err(structure_err)?;
    if stats.is_empty() {
        return Ok(None);
    }
    let chapters = crate::epub_structure::get_chapters(pool, file_id)
        .await
        .map_err(structure_err)?;

    // Fast: the spine step is right there in the CFI string, so the chapter
    // costs nothing. Full: the offset *within* that document needs the same
    // single-file walk the percent derivation runs, off the async runtime
    // since it opens the archive.
    let walked = match (detail, record.epub_cfi.clone()) {
        (PositionDetail::Full, Some(cfi)) => tokio::task::spawn_blocking(move || {
            crate::kobo_position::cfi_spine_offset(&source, &cfi)
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten(),
        (PositionDetail::Fast, Some(ref cfi)) => crate::kobo_position::parse_cfi(cfi)
            .map(|parsed| (parsed.spine_index, 0u64))
            .filter(|(spine_index, _)| *spine_index < stats.len()),
        (_, None) => None,
    };
    // No CFI (a Kobo's percent-only write), or one this build can't anchor:
    // invert the stored percent back onto the spine. Coarse by construction —
    // a percent addresses a point, not the position that produced it.
    let (spine_index, offset, exact) = match walked {
        Some((spine_index, offset)) => (spine_index as i64, offset, true),
        None => {
            let Some(percent) = record.progress_percent.filter(|p| (0..=100).contains(p)) else {
                return Ok(None);
            };
            let Some((spine_index, offset)) =
                crate::epub_structure::position_at_fraction(&stats, percent as f64 / 100.0)
            else {
                return Ok(None);
            };
            (spine_index, offset, false)
        }
    };

    // Under `Fast` the offset is a placeholder zero, so a derived percent would
    // report the *start* of the spine document rather than the reader's place;
    // the stored percent is the honest figure there.
    let percent_through_book = match detail {
        PositionDetail::Full => crate::epub_structure::percent_at(&stats, spine_index, offset)
            .or(record.progress_percent),
        PositionDetail::Fast => record
            .progress_percent
            .or_else(|| crate::epub_structure::percent_at(&stats, spine_index, offset)),
    };
    let placed = chapter_at_spine(&chapters, spine_index);
    // Consecutive TOC entries inside one spine document are indistinguishable
    // at spine granularity, which is all `start_chars` records — so landing in
    // such a document means the chapter named is one of several candidates.
    let ambiguous = placed.is_some_and(|i| {
        chapters[i].spine_index == spine_index
            && chapters
                .iter()
                .filter(|c| c.spine_index == spine_index)
                .count()
                > 1
    });
    let confidence = if exact && !ambiguous {
        PositionConfidence::High
    } else {
        PositionConfidence::Low
    };
    Ok(Some(ResolvedPosition {
        spine_index: Some(spine_index),
        chapter_title: placed.map(|i| chapters[i].title.clone()),
        chapter_ordinal: placed.map(|i| i as i64 + 1),
        chapters_total: (!chapters.is_empty()).then_some(chapters.len() as i64),
        // Meaningless without a real in-document offset — omitted rather than
        // reported as "at the start of the chapter".
        percent_through_chapter: (detail == PositionDetail::Full)
            .then(|| {
                placed.and_then(|i| chapter_span_percent(&stats, &chapters, i, spine_index, offset))
            })
            .flatten(),
        percent_through_book,
        confidence,
    }))
}

/// Index of the chapter covering `spine_index`: the last one that starts at
/// or before it. `None` for a book with no TOC, and for a position ahead of
/// every chapter's start (front matter outside the TOC).
fn chapter_at_spine(chapters: &[EbookChapterRow], spine_index: i64) -> Option<usize> {
    chapters
        .iter()
        .enumerate()
        .filter(|(_, c)| c.spine_index <= spine_index)
        .map(|(i, _)| i)
        .next_back()
}

/// Percent through chapter `index`, measured in the whole-book visible-char
/// coordinate system both `ebook_chapters.start_chars` and `epub_spine_stats`
/// are stored in.
fn chapter_span_percent(
    stats: &[SpineStatRow],
    chapters: &[EbookChapterRow],
    index: usize,
    spine_index: i64,
    offset: u64,
) -> Option<i64> {
    let start = chapters.get(index)?.start_chars.max(0);
    // The next chapter that actually begins later: consecutive TOC entries
    // inside one spine document share a `start_chars`, and a zero-width span
    // would report every position in the file as 0%.
    let end = chapters[index + 1..]
        .iter()
        .map(|c| c.start_chars)
        .find(|&s| s > start)
        .unwrap_or_else(|| {
            stats
                .iter()
                .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)))
        });
    let row = stats.iter().find(|s| s.spine_index == spine_index)?;
    let at = row
        .chars_before
        .max(0)
        .saturating_add(i64::try_from(offset).unwrap_or(i64::MAX));
    span_percent((at - start) as f64, (end - start) as f64)
}

/// `elapsed / span` as a floored 0..=100 percent. `None` for a span that
/// measured nothing — reporting 0% there would claim the reader is at a
/// chapter's start when nothing is known about where they are in it.
fn span_percent(elapsed: f64, span: f64) -> Option<i64> {
    if !span.is_finite() || span <= 0.0 {
        return None;
    }
    let frac = (elapsed / span).clamp(0.0, 1.0);
    Some(((100.0 * frac).floor() as i64).clamp(0, 100))
}

/// Narrow a structure-table read error into this module's error space; the
/// only variant is a wrapped `sqlx::Error`.
fn structure_err(e: crate::epub_structure::EpubStructureError) -> ProgressError {
    match e {
        crate::epub_structure::EpubStructureError::Sqlx(inner) => ProgressError::Sqlx(inner),
    }
}

/// Resolve the audio file for a progress row and measure duration + part
/// position against it. `None` when the book has no resolvable audio file
/// (e.g. every file was removed after the position was saved).
///
/// The row's stored `book_file_id` picks the file for a book carrying more
/// than one audiobook, so the resume card reads out the narration the user
/// was actually in. It is a soft reference (rule 06) — a stale id, or one
/// belonging to another book, falls back to the first audio file by ordinal,
/// which is what the whole feature did before the id was recorded.
async fn audio_totals(
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
        audio_part: mark_index_at(&chapters, position).map(|i| i as i64 + 1),
        audio_part_count: (!chapters.is_empty()).then_some(chapters.len() as i64),
        marks: chapters,
    }))
}

/// 0-based index of the mark covering `elapsed` seconds, mirroring the
/// player's own display arithmetic (not the stored `file_chapters.ordinal`,
/// which is container-supplied and not guaranteed dense).
///
/// `pub(super)` rather than private: exercised directly by a boundary test in
/// `progress::tests` alongside the rest of the resume-card coverage.
pub(super) fn mark_index_at(chapters: &[ChapterInfo], elapsed: f64) -> Option<usize> {
    if chapters.is_empty() {
        return None;
    }
    Some(
        chapters
            .partition_point(|c| c.start_seconds <= elapsed)
            .saturating_sub(1),
    )
}
