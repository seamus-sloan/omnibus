//! Cross-format position mapping: the per-user link record that turns sync
//! on for one dual-format book, the concatenated audio timeline, the linear
//! percent ↔ seconds mapping, and the resume-candidate composition the REST
//! endpoint serves. Follows `kobo_position`'s rule — every failure degrades
//! to no answer, never a wrong one; unlinked books get no answer at all.

use omnibus_shared::cross_format::{
    AlignmentAudioFile, AlignmentAudioPosition, AlignmentEbook, AlignmentEbookChapter,
    AlignmentLink, AlignmentPosition, AlignmentView, CrossFormatCandidate, CrossFormatLinkMode,
    CrossFormatResume, CrossFormatResumeState, MappingConfidence,
};
use omnibus_shared::ProgressFormat;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{progress, resolve_book_id_by_uuid, resolve_canonical_book_uuid};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum CrossFormatError {
    #[error("book not found")]
    BookNotFound,
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
    if !(0..=100).contains(&percent) || timeline.total_seconds <= 0.0 {
        return None;
    }
    let global = timeline.total_seconds * (percent as f64) / 100.0;
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
    let f = timeline
        .files
        .iter()
        .find(|f| f.book_file_id == book_file_id)?;
    if timeline.total_seconds <= 0.0 {
        return None;
    }
    let global = f.start_seconds + seconds_within.clamp(0.0, f.duration_seconds);
    Some(((100.0 * global / timeline.total_seconds).floor() as i64).clamp(0, 100))
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

    let candidate = match target {
        ProgressFormat::Audio => source.progress_percent.and_then(|pct| {
            map_percent_to_audio(&timeline, pct).map(|(file_id, seconds)| CrossFormatCandidate {
                target,
                source_format,
                source_client_updated_at: source.client_updated_at,
                confidence: MappingConfidence::Linear,
                book_file_id: Some(file_id),
                audio_position_seconds: Some(seconds),
                total_duration_seconds: Some(timeline.total_seconds),
                percent: None,
            })
        }),
        ProgressFormat::Epub => source.audio_position_seconds.and_then(|seconds| {
            // The row's own file when recorded. A file-less row is only
            // unambiguous on a single-file timeline — guessing the first
            // of several would map another file's offset, so refuse.
            let file_id = source
                .book_file_id
                .or_else(|| (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id))?;
            map_audio_to_percent(&timeline, file_id, seconds).map(|pct| CrossFormatCandidate {
                target,
                source_format,
                source_client_updated_at: source.client_updated_at,
                confidence: MappingConfidence::Linear,
                book_file_id: None,
                audio_position_seconds: None,
                total_duration_seconds: None,
                percent: Some(pct),
            })
        }),
    };
    Ok(match candidate {
        Some(candidate) => CrossFormatResume {
            state: CrossFormatResumeState::Candidate,
            candidate: Some(candidate),
        },
        None => CrossFormatResume::empty(CrossFormatResumeState::NothingNewer),
    })
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

    let link = match get_link(pool, user_id, &book_uuid).await? {
        Some(link) => {
            let stale = link_is_stale(pool, book_id, &link).await?;
            Some(AlignmentLink {
                mode: link.mode,
                primary_book_file_id: link.primary_book_file_id,
                stale,
                confirmed_at: link.confirmed_at,
            })
        }
        None => None,
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

    let mut audio = Vec::new();
    for f in audio_files(pool, book_id).await? {
        let chapter_starts = crate::hls::get_chapters(pool, f.book_file_id)
            .await?
            .into_iter()
            .map(|c| c.start_seconds)
            .collect();
        audio.push(AlignmentAudioFile {
            book_file_id: f.book_file_id,
            label: f.scan_key.clone(),
            duration_seconds: f.duration_seconds,
            chapter_starts,
        });
    }

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
        ebook,
        audio_files: audio,
        reading,
        listening,
    })
}

/// Persist a re-ordering of the book's audio files. `order` must name
/// exactly the book's current audio file set — anything else refuses,
/// so a stale modal can't scramble ordinals after a re-index. Two-phase
/// ordinal write because `UNIQUE(book_id, format, ordinal)` is enforced
/// per row.
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
        return Err(CrossFormatError::BookNotFound);
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
