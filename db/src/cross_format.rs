//! Cross-format position mapping: the per-user link record that turns sync
//! on for one dual-format book, the concatenated audio timeline, the linear
//! percent ↔ seconds mapping, and the resume-candidate composition the REST
//! endpoint serves. Follows `kobo_position`'s rule — every failure degrades
//! to no answer, never a wrong one; unlinked books get no answer at all.

use omnibus_shared::cross_format::{
    AlignmentAudioFile, AlignmentAudioPosition, AlignmentEbook, AlignmentEbookChapter,
    AlignmentLink, AlignmentMatch, AlignmentPosition, AlignmentView, CrossFormatCandidate,
    CrossFormatLinkMode, CrossFormatResume, CrossFormatResumeState, MappingConfidence,
};
use omnibus_shared::ProgressFormat;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{progress, resolve_book_id_by_uuid, resolve_canonical_book_uuid};

mod anchors;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum CrossFormatError {
    #[error("book not found")]
    BookNotFound,
    #[error("audio files changed since the alignment loaded")]
    AudioSetMismatch,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for CrossFormatError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => CrossFormatError::Sqlx(inner),
            // This module never decodes overrides JSON; fold the variant
            // rather than panic so a future caller can't slip through.
            crate::books::BooksError::OverridesJson(inner) => {
                CrossFormatError::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

impl From<crate::epub_structure::EpubStructureError> for CrossFormatError {
    fn from(e: crate::epub_structure::EpubStructureError) -> Self {
        match e {
            crate::epub_structure::EpubStructureError::Sqlx(inner) => CrossFormatError::Sqlx(inner),
        }
    }
}

impl From<crate::hls::HlsError> for CrossFormatError {
    fn from(e: crate::hls::HlsError) -> Self {
        match e {
            crate::hls::HlsError::Db(inner) => CrossFormatError::Sqlx(inner),
        }
    }
}

impl From<progress::ProgressError> for CrossFormatError {
    fn from(e: progress::ProgressError) -> Self {
        match e {
            progress::ProgressError::BookNotFound => CrossFormatError::BookNotFound,
            progress::ProgressError::Sqlx(inner) => CrossFormatError::Sqlx(inner),
        }
    }
}

/// One confirmed link row — the user's declaration that this book's
/// formats describe the same text and may sync.
#[derive(Debug, Clone, PartialEq)]
pub struct CrossFormatLink {
    pub mode: CrossFormatLinkMode,
    pub primary_book_file_id: Option<i64>,
    pub audio_snapshot: String,
    pub confirmed_at: i64,
}

fn mode_str(mode: CrossFormatLinkMode) -> &'static str {
    match mode {
        CrossFormatLinkMode::Sequence => "sequence",
        CrossFormatLinkMode::Narrations => "narrations",
    }
}

fn parse_mode(raw: &str) -> CrossFormatLinkMode {
    // The CHECK constraint rules out anything else; an unknown string
    // defaults to narrations, the maximally conservative reading — with
    // no primary recorded it refuses to map at all, rather than
    // concatenating files the user never declared sequential.
    match raw {
        "sequence" => CrossFormatLinkMode::Sequence,
        _ => CrossFormatLinkMode::Narrations,
    }
}

/// Confirm (or re-confirm) a link. Snapshots the book's current audio file
/// set so later file changes read as stale, and replaces any existing row
/// wholesale — a re-confirmation is a fresh declaration, not a merge.
pub async fn upsert_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    mode: CrossFormatLinkMode,
    primary_book_file_id: Option<i64>,
) -> Result<CrossFormatLink, CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let snapshot = snapshot_json(&audio_files(pool, book_id).await?);
    sqlx::query(
        "INSERT INTO cross_format_links
            (user_id, book_uuid, mode, primary_book_file_id, audio_snapshot, confirmed_at)
         VALUES (?, ?, ?, ?, ?, CAST(strftime('%s','now') AS INTEGER))
         ON CONFLICT(user_id, book_uuid) DO UPDATE SET
            mode = excluded.mode,
            primary_book_file_id = excluded.primary_book_file_id,
            audio_snapshot = excluded.audio_snapshot,
            confirmed_at = excluded.confirmed_at",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(mode_str(mode))
    .bind(primary_book_file_id)
    .bind(&snapshot)
    .execute(pool)
    .await?;
    get_link(pool, user_id, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)
}

/// The stored link for `(user, book)`, if the user has confirmed one.
pub async fn get_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<CrossFormatLink>, CrossFormatError> {
    let Some(book_uuid) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let row: Option<(String, Option<i64>, String, i64)> = sqlx::query_as(
        "SELECT mode, primary_book_file_id, audio_snapshot, confirmed_at
         FROM cross_format_links WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(mode, primary_book_file_id, audio_snapshot, confirmed_at)| CrossFormatLink {
            mode: parse_mode(&mode),
            primary_book_file_id,
            audio_snapshot,
            confirmed_at,
        },
    ))
}

/// Remove a link (sync off). Returns whether a row existed.
pub async fn delete_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<bool, CrossFormatError> {
    let Some(book_uuid) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(false);
    };
    let result = sqlx::query("DELETE FROM cross_format_links WHERE user_id = ? AND book_uuid = ?")
        .bind(user_id)
        .bind(&book_uuid)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// One audio file of the book with its summed part duration, in ordinal
/// order — the raw material for both the timeline and the snapshot.
#[derive(Debug, Clone)]
struct AudioFile {
    book_file_id: i64,
    scan_key: String,
    size_bytes: i64,
    mtime_epoch: i64,
    duration_seconds: f64,
}

async fn audio_files(pool: &SqlitePool, book_id: i64) -> Result<Vec<AudioFile>, sqlx::Error> {
    let rows: Vec<(i64, String, i64, i64, f64)> = sqlx::query_as(
        "SELECT bf.id, COALESCE(bf.scan_key, ''), bf.size_bytes, bf.mtime_epoch,
                COALESCE(SUM(bfp.duration_seconds), 0)
         FROM book_files bf
         LEFT JOIN book_file_parts bfp ON bfp.book_file_id = bf.id
         WHERE bf.book_id = ? AND bf.format IN ('M4B', 'M4A', 'MP3')
         GROUP BY bf.id
         ORDER BY bf.ordinal, bf.id",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(book_file_id, scan_key, size_bytes, mtime_epoch, duration_seconds)| AudioFile {
                book_file_id,
                scan_key,
                size_bytes,
                mtime_epoch,
                duration_seconds,
            },
        )
        .collect())
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct SnapshotEntry {
    scan_key: String,
    etag: Option<String>,
}

/// Canonical JSON of an audio file set: `(scan_key, wire etag)` pairs
/// sorted by scan_key, so comparison is order- and id-independent —
/// `book_files.id` changes on every Changed re-index, scan_key doesn't.
fn snapshot_json(files: &[AudioFile]) -> String {
    let mut entries: Vec<SnapshotEntry> = files
        .iter()
        .map(|f| SnapshotEntry {
            scan_key: f.scan_key.clone(),
            etag: omnibus_shared::file_etag(f.size_bytes, f.mtime_epoch),
        })
        .collect();
    entries.sort();
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}

/// Whether the book's current audio file set no longer matches what the
/// user confirmed — a replaced, added, or removed file pauses mapping.
pub async fn link_is_stale(
    pool: &SqlitePool,
    book_id: i64,
    link: &CrossFormatLink,
) -> Result<bool, CrossFormatError> {
    let current = snapshot_json(&audio_files(pool, book_id).await?);
    Ok(current != link.audio_snapshot)
}

/// The virtual concatenated audio timeline mapping runs against: files in
/// ordinal order with cumulative start offsets (sequence mode), or the
/// primary file alone (narrations mode — nothing concatenates across
/// alternate recordings).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTimeline {
    pub files: Vec<TimelineFile>,
    pub total_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineFile {
    pub book_file_id: i64,
    pub start_seconds: f64,
    pub duration_seconds: f64,
}

/// Build the timeline for a linked book. `Ok(None)` when it can't be
/// trusted: no audio files, a narrations link whose primary no longer
/// exists, or a zero-length total.
pub async fn audio_timeline(
    pool: &SqlitePool,
    book_id: i64,
    link: &CrossFormatLink,
) -> Result<Option<AudioTimeline>, CrossFormatError> {
    let all = audio_files(pool, book_id).await?;
    let selected: Vec<AudioFile> = match link.mode {
        CrossFormatLinkMode::Sequence => all,
        CrossFormatLinkMode::Narrations => {
            let Some(primary) = link.primary_book_file_id else {
                return Ok(None);
            };
            all.into_iter()
                .filter(|f| f.book_file_id == primary)
                .collect()
        }
    };
    if selected.is_empty() {
        return Ok(None);
    }
    let mut files = Vec::with_capacity(selected.len());
    let mut start = 0.0f64;
    for f in &selected {
        files.push(TimelineFile {
            book_file_id: f.book_file_id,
            start_seconds: start,
            duration_seconds: f.duration_seconds.max(0.0),
        });
        start += f.duration_seconds.max(0.0);
    }
    if start <= 0.0 {
        return Ok(None);
    }
    Ok(Some(AudioTimeline {
        files,
        total_seconds: start,
    }))
}

/// Linear tier: whole-book percent → `(book_file_id, seconds-within-file)`
/// — the shape `reading_progress` stores for audio. `None` for anything
/// out of range rather than a clamped guess at the wrong file.
pub fn map_percent_to_audio(timeline: &AudioTimeline, percent: i64) -> Option<(i64, f64)> {
    if !(0..=100).contains(&percent) {
        return None;
    }
    map_fraction_to_audio(timeline, (percent as f64) / 100.0)
}

/// Fraction-level half of [`map_percent_to_audio`], shared with the
/// anchored tier (which interpolates the fraction first).
fn map_fraction_to_audio(timeline: &AudioTimeline, frac: f64) -> Option<(i64, f64)> {
    if !(0.0..=1.0).contains(&frac) || timeline.total_seconds <= 0.0 {
        return None;
    }
    let global = timeline.total_seconds * frac;
    let last_idx = timeline.files.len().checked_sub(1)?;
    for (i, f) in timeline.files.iter().enumerate() {
        let end = f.start_seconds + f.duration_seconds;
        // The end boundary belongs to the next file, except at 100% where
        // it belongs to the last file so the position never lands past it.
        if global < end || i == last_idx {
            return Some((
                f.book_file_id,
                (global - f.start_seconds).clamp(0.0, f.duration_seconds),
            ));
        }
    }
    None
}

/// Linear tier inverse: a position within one timeline file → whole-book
/// percent (floor, clamped). `None` when the file isn't on the timeline —
/// e.g. a non-primary narration, which is deliberately unaligned.
pub fn map_audio_to_percent(
    timeline: &AudioTimeline,
    book_file_id: i64,
    seconds_within: f64,
) -> Option<i64> {
    let frac = audio_fraction(timeline, book_file_id, seconds_within)?;
    Some(((100.0 * frac).floor() as i64).clamp(0, 100))
}

/// Fraction-level half of [`map_audio_to_percent`], shared with the
/// anchored tier so interpolation runs before the floor loses precision.
fn audio_fraction(timeline: &AudioTimeline, book_file_id: i64, seconds_within: f64) -> Option<f64> {
    let f = timeline
        .files
        .iter()
        .find(|f| f.book_file_id == book_file_id)?;
    if timeline.total_seconds <= 0.0 {
        return None;
    }
    let global = f.start_seconds + seconds_within.clamp(0.0, f.duration_seconds);
    Some((global / timeline.total_seconds).clamp(0.0, 1.0))
}

/// Full-precision whole-book fraction of a reading row: derived from the
/// stored CFI via spine stats when both exist (sub-percent, the same
/// machinery as `derive_epub_percent`), else the stored integer percent.
/// Every failure along the precise path — no stats yet, unreadable file,
/// unanchorable CFI — degrades to the integer fallback rather than
/// erroring: the caller is composing an offer, and a coarser offer beats
/// no answer.
async fn epub_source_fraction(
    pool: &SqlitePool,
    book_id: i64,
    source: &omnibus_shared::ProgressRecord,
) -> Option<f64> {
    let fallback = source
        .progress_percent
        .filter(|p| (0..=100).contains(p))
        .map(|p| p as f64 / 100.0);
    let Some(cfi) = source.epub_cfi.clone() else {
        return fallback;
    };
    let precise = async {
        let (file_id, epub_path) = crate::book_file_with_id(pool, book_id, "EPUB")
            .await
            .ok()??;
        let stats = crate::epub_structure::get_spine_stats(pool, file_id)
            .await
            .ok()?;
        if stats.is_empty() {
            return None;
        }
        let (spine_index, offset) = tokio::task::spawn_blocking(move || {
            crate::kobo_position::cfi_spine_offset(&epub_path, &cfi)
        })
        .await
        .ok()?
        .ok()??;
        crate::epub_structure::fraction_at(&stats, spine_index as i64, offset)
    }
    .await;
    precise.or(fallback)
}

/// Compose the answer for `GET /api/books/{uuid}/cross-format-resume`:
/// gate on the link (off until confirmed, paused while stale), compare the
/// two rows' ordering clocks, and map the newer source onto `target`.
/// Anything unmappable degrades to `NothingNewer` — the endpoint never
/// invents a position.
pub async fn resume_candidate(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    target: ProgressFormat,
) -> Result<CrossFormatResume, CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;

    let Some(link) = get_link(pool, user_id, &book_uuid).await? else {
        return Ok(CrossFormatResume::empty(CrossFormatResumeState::NotLinked));
    };
    if link_is_stale(pool, book_id, &link).await? {
        return Ok(CrossFormatResume::empty(CrossFormatResumeState::LinkStale));
    }

    let source_format = match target {
        ProgressFormat::Audio => ProgressFormat::Epub,
        ProgressFormat::Epub => ProgressFormat::Audio,
    };
    let source = progress::get_progress(pool, user_id, &book_uuid, source_format).await?;
    let target_row = progress::get_progress(pool, user_id, &book_uuid, target).await?;
    let Some(source) = source else {
        return Ok(CrossFormatResume::empty(
            CrossFormatResumeState::NothingNewer,
        ));
    };
    if let Some(t) = &target_row {
        if source.client_updated_at <= t.client_updated_at {
            return Ok(CrossFormatResume::empty(
                CrossFormatResumeState::NothingNewer,
            ));
        }
    }
    let Some(timeline) = audio_timeline(pool, book_id, &link).await? else {
        return Ok(CrossFormatResume::empty(
            CrossFormatResumeState::NothingNewer,
        ));
    };

    // The anchored tier engages when the served EPUB has extracted
    // structure and its chapters match the audio marks; otherwise every
    // path below falls through to the linear tier.
    let anchor_map = match crate::book_file_with_id(pool, book_id, "EPUB").await? {
        Some((ebook_file_id, _)) => anchors::anchor_map(pool, ebook_file_id, &timeline).await?,
        None => None,
    };
    let confidence = if anchor_map.is_some() {
        MappingConfidence::ChapterAnchored
    } else {
        MappingConfidence::Linear
    };
    // The gate only engages against an existing target position — skip the
    // stats query entirely on first-open targets.
    let tol_frac = match &target_row {
        Some(_) => equivalence_fraction(epub_total_chars(pool, book_id).await),
        None => equivalence_fraction(None),
    };
    let (candidate, aligned) = match target {
        ProgressFormat::Audio => match epub_source_fraction(pool, book_id, &source).await {
            None => (None, false),
            Some(src_frac) => {
                let frac = match &anchor_map {
                    Some(map) => anchors::interpolate(&map.anchors, src_frac, true),
                    None => src_frac,
                };
                match map_fraction_to_audio(&timeline, frac) {
                    None => (None, false),
                    Some((file_id, seconds)) => {
                        let mapped_global = timeline
                            .files
                            .iter()
                            .find(|f| f.book_file_id == file_id)
                            .map(|f| f.start_seconds + seconds)
                            .unwrap_or(frac * timeline.total_seconds);
                        // The target's own spot on the same timeline. A
                        // file-less multi-file row can't be placed, so the
                        // gate stands aside and the offer survives.
                        let current_global = target_row.as_ref().and_then(|t| {
                            let secs = t.audio_position_seconds?;
                            let file = t.book_file_id.or_else(|| {
                                (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id)
                            })?;
                            audio_fraction(&timeline, file, secs)
                                .map(|f| f * timeline.total_seconds)
                        });
                        let tol = (tol_frac * timeline.total_seconds)
                            .max(audio_equivalence_floor(timeline.total_seconds));
                        let aligned =
                            current_global.is_some_and(|cur| (mapped_global - cur).abs() <= tol);
                        let candidate = CrossFormatCandidate {
                            target,
                            source_format,
                            source_client_updated_at: source.client_updated_at,
                            confidence,
                            book_file_id: Some(file_id),
                            audio_position_seconds: Some(seconds),
                            total_duration_seconds: Some(timeline.total_seconds),
                            percent: None,
                            fraction: None,
                            source_ahead: current_global.map(|cur| mapped_global > cur),
                        };
                        (Some(candidate), aligned)
                    }
                }
            }
        },
        ProgressFormat::Epub => {
            // The row's own file when recorded. A file-less row is only
            // unambiguous on a single-file timeline — guessing the first
            // of several would map another file's offset, so refuse.
            let mapped = source.audio_position_seconds.and_then(|seconds| {
                let file_id = source.book_file_id.or_else(|| {
                    (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id)
                })?;
                audio_fraction(&timeline, file_id, seconds)
            });
            match mapped {
                None => (None, false),
                Some(raw_frac) => {
                    let frac = match &anchor_map {
                        Some(map) => anchors::interpolate(&map.anchors, raw_frac, false),
                        None => raw_frac,
                    };
                    let pct = ((100.0 * frac).floor() as i64).clamp(0, 100);
                    let current = match &target_row {
                        Some(t) => epub_source_fraction(pool, book_id, t).await,
                        None => None,
                    };
                    let aligned = current.is_some_and(|cur| (frac - cur).abs() <= tol_frac);
                    let candidate = CrossFormatCandidate {
                        target,
                        source_format,
                        source_client_updated_at: source.client_updated_at,
                        confidence,
                        book_file_id: None,
                        audio_position_seconds: None,
                        total_duration_seconds: None,
                        percent: Some(pct),
                        fraction: Some(frac.clamp(0.0, 1.0)),
                        source_ahead: current.map(|cur| frac > cur),
                    };
                    (Some(candidate), aligned)
                }
            }
        }
    };
    Ok(match candidate {
        // Same spot within tolerance: the mapped position still rides
        // along (the hero's format-switch CTA wants it) but the state
        // tells prompt surfaces to stay quiet — a jump would move the
        // user nowhere, or worse, one quantization step backward.
        Some(candidate) if aligned => CrossFormatResume {
            state: CrossFormatResumeState::Aligned,
            candidate: Some(candidate),
        },
        Some(candidate) => CrossFormatResume {
            state: CrossFormatResumeState::Candidate,
            candidate: Some(candidate),
        },
        None => CrossFormatResume::empty(CrossFormatResumeState::NothingNewer),
    })
}

/// Positions closer than this fraction of the whole book count as "the
/// same spot" — an offer inside it is quantization noise, not signal.
/// Base 0.5%, widened to two reader locations (~1024 visible chars each,
/// the epub.js jump granularity) on short books where one location is a
/// large slice, and capped at 5% so a tiny book can't suppress every
/// offer outright.
fn equivalence_fraction(total_chars: Option<i64>) -> f64 {
    const BASE: f64 = 0.005;
    const READER_LOCATION_CHARS: f64 = 1024.0;
    const CAP: f64 = 0.05;
    let location_slack = total_chars
        .filter(|c| *c > 0)
        .map(|c| 2.0 * READER_LOCATION_CHARS / c as f64)
        .unwrap_or(0.0);
    BASE.max(location_slack).min(CAP)
}

/// Audio-side floor: sub-minute deltas are seek/heartbeat noise on a real
/// audiobook — but the floor never exceeds 2% of the timeline, so a short
/// book keeps meaningful offers instead of counting everything as aligned.
fn audio_equivalence_floor(total_seconds: f64) -> f64 {
    60.0f64.min(0.02 * total_seconds.max(0.0))
}

/// Total visible chars of the served EPUB's extracted spine stats;
/// `None` until the structure backfill has reached the book.
async fn epub_total_chars(pool: &SqlitePool, book_id: i64) -> Option<i64> {
    let (file_id, _) = crate::book_file_with_id(pool, book_id, "EPUB")
        .await
        .ok()??;
    let stats = crate::epub_structure::get_spine_stats(pool, file_id)
        .await
        .ok()?;
    let total = stats
        .iter()
        .fold(0i64, |a, s| a.saturating_add(s.visible_chars.max(0)));
    (total > 0).then_some(total)
}

/// Assemble everything the alignment modal renders in one read: link +
/// staleness, ebook chapter ticks over the stored spine stats, per-file
/// audio segments with chapter offsets, and both current positions.
pub async fn alignment_view(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<AlignmentView, CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;

    let raw_link = get_link(pool, user_id, &book_uuid).await?;
    let link = match &raw_link {
        Some(link) => {
            let stale = link_is_stale(pool, book_id, link).await?;
            Some(AlignmentLink {
                mode: link.mode,
                primary_book_file_id: link.primary_book_file_id,
                stale,
                confirmed_at: link.confirmed_at,
            })
        }
        None => None,
    };
    // Anchor preview: for an unlinked book, evaluate against the default
    // sequence declaration so the modal can honestly say whether anchoring
    // would engage; a stale link stays quiet (mapping is paused anyway).
    let anchor_match = if link.as_ref().is_some_and(|l| l.stale) {
        None
    } else {
        let preview = raw_link.clone().unwrap_or(CrossFormatLink {
            mode: CrossFormatLinkMode::Sequence,
            primary_book_file_id: None,
            audio_snapshot: String::new(),
            confirmed_at: 0,
        });
        match audio_timeline(pool, book_id, &preview).await? {
            Some(timeline) => match crate::book_file_with_id(pool, book_id, "EPUB").await? {
                Some((ebook_file_id, _)) => anchors::anchor_map(pool, ebook_file_id, &timeline)
                    .await?
                    .map(|m| AlignmentMatch {
                        matched: m.matched,
                        ebook_chapters: m.ebook_chapters,
                        confidence: MappingConfidence::ChapterAnchored,
                    }),
                None => None,
            },
            None => None,
        }
    };

    // Text lane: stored spine stats (0071). Absent until the structure
    // backfill reaches the book — the modal shows the honest linear
    // notice in that case rather than fabricated ticks.
    let ebook = match crate::book_file_with_id(pool, book_id, "EPUB").await? {
        Some((file_id, _)) => {
            let stats = crate::epub_structure::get_spine_stats(pool, file_id).await?;
            let total: i64 = stats
                .iter()
                .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
            if stats.is_empty() || total <= 0 {
                None
            } else {
                let chapters = crate::epub_structure::get_chapters(pool, file_id)
                    .await?
                    .into_iter()
                    .map(|c| AlignmentEbookChapter {
                        title: c.title,
                        percent: (100.0 * c.start_chars.max(0) as f64 / total as f64)
                            .clamp(0.0, 100.0),
                    })
                    .collect();
                Some(AlignmentEbook {
                    total_chars: total,
                    chapters,
                })
            }
        }
        None => None,
    };

    let files = audio_files(pool, book_id).await?;
    let ids: Vec<i64> = files.iter().map(|f| f.book_file_id).collect();
    let mut starts_by_file = bulk_chapter_starts(pool, &ids).await?;
    let audio = files
        .into_iter()
        .map(|f| AlignmentAudioFile {
            book_file_id: f.book_file_id,
            label: f.scan_key,
            duration_seconds: f.duration_seconds,
            chapter_starts: starts_by_file.remove(&f.book_file_id).unwrap_or_default(),
        })
        .collect();

    let reading = progress::get_progress(pool, user_id, &book_uuid, ProgressFormat::Epub)
        .await?
        .map(|r| AlignmentPosition {
            percent: r.progress_percent,
            client_updated_at: r.client_updated_at,
        });
    let listening = progress::get_progress(pool, user_id, &book_uuid, ProgressFormat::Audio)
        .await?
        .and_then(|r| {
            r.audio_position_seconds
                .map(|seconds| AlignmentAudioPosition {
                    book_file_id: r.book_file_id,
                    seconds,
                    client_updated_at: r.client_updated_at,
                })
        });

    Ok(AlignmentView {
        link,
        anchor_match,
        ebook,
        audio_files: audio,
        reading,
        listening,
    })
}

/// Chapter start offsets for a set of audio files in one query (the
/// modal's lane ticks), grouped in memory — one round trip instead of
/// one per file. Chunked under SQLite's bind cap.
async fn bulk_chapter_starts(
    pool: &SqlitePool,
    book_file_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<f64>>, sqlx::Error> {
    let mut map: std::collections::HashMap<i64, Vec<f64>> = std::collections::HashMap::new();
    for chunk in book_file_ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT book_file_id, start_seconds FROM file_chapters
             WHERE book_file_id IN ({placeholders})
             ORDER BY book_file_id, start_seconds"
        );
        let mut q = sqlx::query_as::<_, (i64, f64)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for (file_id, start) in q.fetch_all(pool).await? {
            map.entry(file_id).or_default().push(start);
        }
    }
    Ok(map)
}

/// Persist a re-ordering of the book's audio files. `order` must name
/// exactly the book's current audio file set — anything else refuses with
/// [`CrossFormatError::AudioSetMismatch`], so a stale modal can't scramble
/// ordinals after a re-index. Two-phase ordinal write because
/// `UNIQUE(book_id, format, ordinal)` is enforced per row.
pub async fn set_audio_order(
    pool: &SqlitePool,
    book_uuid: &str,
    order: &[i64],
) -> Result<(), CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let current: std::collections::HashSet<i64> = audio_files(pool, book_id)
        .await?
        .into_iter()
        .map(|f| f.book_file_id)
        .collect();
    let proposed: std::collections::HashSet<i64> = order.iter().copied().collect();
    if proposed.len() != order.len() || proposed != current {
        return Err(CrossFormatError::AudioSetMismatch);
    }
    let mut tx = pool.begin().await?;
    for (i, id) in order.iter().enumerate() {
        sqlx::query("UPDATE book_files SET ordinal = ? WHERE id = ? AND book_id = ?")
            .bind(-(i as i64) - 1)
            .bind(id)
            .bind(book_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "UPDATE book_files SET ordinal = -ordinal - 1
         WHERE book_id = ? AND format IN ('M4B', 'M4A', 'MP3') AND ordinal < 0",
    )
    .bind(book_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
