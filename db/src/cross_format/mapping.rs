//! The linear tier: the book's audio file set, the snapshot that detects
//! it changing, the virtual concatenated timeline, and the percent ↔
//! seconds ↔ fraction conversions every other tier is measured against.

use omnibus_shared::cross_format::CrossFormatLinkMode;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::{CrossFormatError, CrossFormatLink};

/// One audio file of the book with its summed part duration, in ordinal
/// order — the raw material for both the timeline and the snapshot.
#[derive(Debug, Clone)]
pub(super) struct AudioFile {
    pub(super) book_file_id: i64,
    pub(super) scan_key: String,
    pub(super) size_bytes: i64,
    pub(super) mtime_epoch: i64,
    pub(super) duration_seconds: f64,
}

pub(super) async fn audio_files<'e, E>(exec: E, book_id: i64) -> Result<Vec<AudioFile>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
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
    .fetch_all(exec)
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

/// Canonical JSON of an audio file set: `(scan_key, wire etag)` pairs in
/// **ordinal order**. Keyed on scan_key + etag so comparison is
/// id-independent (`book_files.id` changes on every Changed re-index,
/// scan_key doesn't) — but deliberately order-DEPENDENT: the sequence
/// timeline concatenates by ordinal, so a re-order changes what every
/// stored global second means and must read as a different set (stale
/// links, cleared anchors). A sorted snapshot let `set_audio_order`
/// permute the timeline under every user's live mapping undetected.
pub(super) fn snapshot_json(files: &[AudioFile]) -> String {
    let entries: Vec<SnapshotEntry> = files
        .iter()
        .map(|f| SnapshotEntry {
            scan_key: f.scan_key.clone(),
            etag: omnibus_shared::file_etag(f.size_bytes, f.mtime_epoch),
        })
        .collect();
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
pub(super) fn map_fraction_to_audio(timeline: &AudioTimeline, frac: f64) -> Option<(i64, f64)> {
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
pub(super) fn audio_fraction(
    timeline: &AudioTimeline,
    book_file_id: i64,
    seconds_within: f64,
) -> Option<f64> {
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
pub(super) async fn epub_source_fraction(
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
    fraction_for_cfi(pool, book_id, cfi).await.or(fallback)
}

/// Full-precision whole-book fraction of one CFI via spine stats — the
/// single ruler every cross-format exchange resolves on. `None` on any
/// missing input (no stats yet, unreadable file, unanchorable CFI).
pub(super) async fn fraction_for_cfi(pool: &SqlitePool, book_id: i64, cfi: String) -> Option<f64> {
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

/// Derive the point CFI at a whole-book fraction — the jump target the web
/// reader consumes directly, so the fraction is never reinterpreted on
/// epub.js's different locations scale. Response-only and on request (the
/// resume endpoint's `?derive=cfi`); `None` degrades to the fraction path.
pub async fn derive_candidate_cfi(pool: &SqlitePool, book_uuid: &str, frac: f64) -> Option<String> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid).await.ok()??;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid).await.ok()??;
    let (file_id, epub_path) = crate::book_file_with_id(pool, book_id, "EPUB")
        .await
        .ok()??;
    let stats = crate::epub_structure::get_spine_stats(pool, file_id)
        .await
        .ok()?;
    let (spine_index, offset) = crate::epub_structure::position_at_fraction(&stats, frac)?;
    tokio::task::spawn_blocking(move || {
        crate::kobo_position::cfi_at_spine_offset(&epub_path, spine_index as usize, offset)
    })
    .await
    .ok()?
    .ok()
    .flatten()
}
