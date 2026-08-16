//! Resume-card read path: [`recent_progress`] plus book/duration/chapter
//! enrichment into [`resume_points`], and the audio-file/chapter-position
//! helpers behind it.

use omnibus_shared::{ChapterInfo, ProgressFormat, ProgressRecord, ResumePoint};
use sqlx::{Row, SqlitePool};

use crate::hls;

use super::{parse_format, ProgressError};

/// The user's most recent progress rows across both formats, newest first
/// by **client event time** — `COALESCE(client_updated_at, updated_at)`, so
/// a queued offline replay landing late doesn't jump a week-old book to the
/// top. Feeds the "pick up where you left off" surfaces via
/// [`resume_points`].
///
/// Books the user has marked `unread` or `finished` are excluded: both are
/// statements that they are not mid-book, and a completed book otherwise sat
/// at the top of the rail until five newer positions pushed it off.
///
/// A **missing** `book_read_status` row keeps its position. 0046 treats
/// absence as `unread` everywhere else, and this is the deliberate exception:
/// absence here means "nothing has been said", not "not reading". Every
/// status write is best-effort (`read_status_auto`, the players), so hiding on
/// absence would drop a book off the rail whenever one of those requests was
/// lost — and would hide every row written before that auto-transition
/// existed.
///
/// The join is per-book while progress is per-`(book, format)`, so finishing
/// a dual-format book clears both its reading and its listening card. Read
/// status is a fact about the book, not about one of its files.
pub async fn recent_progress(
    pool: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> Result<Vec<ProgressRecord>, ProgressError> {
    let rows = sqlx::query(
        "SELECT rp.book_uuid, rp.format, rp.epub_cfi, rp.audio_position_seconds,
                rp.progress_percent, rp.kobo_location, rp.book_file_id,
                rp.updated_at,
                COALESCE(rp.client_updated_at, rp.updated_at) AS client_updated_at
         FROM reading_progress rp
         LEFT JOIN book_read_status rs
                ON rs.user_id = rp.user_id AND rs.book_uuid = rp.book_uuid
         WHERE rp.user_id = ?
           AND (rs.status IS NULL OR rs.status = 'reading')
         ORDER BY COALESCE(rp.client_updated_at, rp.updated_at) DESC, rp.book_uuid
         LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ProgressRecord {
                book_uuid: row.try_get::<String, _>("book_uuid")?,
                format: parse_format(row.try_get::<String, _>("format")?.as_str()),
                epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
                audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
                progress_percent: row.try_get::<Option<i64>, _>("progress_percent")?,
                kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
                book_file_id: row.try_get::<Option<i64>, _>("book_file_id")?,
                updated_at: row.try_get::<i64, _>("updated_at")?,
                client_updated_at: row.try_get::<i64, _>("client_updated_at")?,
            })
        })
        .collect()
}

/// [`recent_progress`] joined with book metadata and, for audio rows, the
/// whole-book duration + chapter position. Rows whose book has since been
/// deleted (progress soft-references `books.uuid` — no cascade) are skipped
/// rather than erroring, so one ghosted book can't blank the landing card.
pub async fn resume_points(
    pool: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> Result<Vec<ResumePoint>, ProgressError> {
    let records = recent_progress(pool, user_id, limit).await?;
    let mut points = Vec::with_capacity(records.len());
    for mut record in records {
        let Some(book) = crate::get_book_by_uuid(pool, &record.book_uuid).await? else {
            continue;
        };
        let audio = match record.format {
            ProgressFormat::Audio => audio_totals(pool, &record.book_uuid, &record).await?,
            ProgressFormat::Epub => None,
        };
        let (total_duration_seconds, chapter_number, chapter_count) = match audio {
            Some(totals) => {
                // Overwrite rather than trust the stored id: it may name a
                // `book_files` row the reindex has since replaced, and the
                // Continue CTA links straight at `?file_id=` — a dead id
                // would open the player on a manifest that 404s.
                record.book_file_id = Some(totals.book_file_id);
                (
                    Some(totals.total_duration_seconds),
                    totals.chapter_number,
                    totals.chapter_count,
                )
            }
            None => {
                // Audio row whose book has no audio file left (and every
                // epub row): drop the stored id rather than hand a CTA an
                // id that resolves to nothing.
                record.book_file_id = None;
                (None, None, None)
            }
        };
        // Only rows that will render a listening card need the preference —
        // it feeds the rate-adjusted "left" readout on the resume surfaces.
        // Direct lookup, not `get_playback_rate`: `record.book_uuid` was
        // canonicalized by `upsert_progress_tx` at write time, so re-resolving
        // per row would double this endpoint's query count for a no-op. A row
        // predating a later merge can miss the preference and fall back to 1x
        // until its next write re-canonicalizes it — cosmetic, and accepted.
        let playback_rate = match total_duration_seconds {
            Some(_) => sqlx::query(
                "SELECT playback_rate FROM audiobook_playback_preferences
                 WHERE user_id = ? AND book_uuid = ?",
            )
            .bind(user_id)
            .bind(&record.book_uuid)
            .fetch_optional(pool)
            .await?
            .map(|row| row.try_get::<f64, _>("playback_rate"))
            .transpose()?,
            None => None,
        };
        points.push(ResumePoint {
            record,
            book,
            linked: false,
            cross_format: None,
            total_duration_seconds,
            chapter_number,
            chapter_count,
            playback_rate,
        });
    }
    collapse_linked_points(pool, user_id, &mut points).await?;
    Ok(points)
}

/// Cross-format post-pass: a linked dual-format book competes with itself
/// when both rows are recent — keep the newer card, mark it linked, and
/// attach the mapped "resume in the other format" candidate. Unlinked
/// books keep today's two-card behavior; a stale link still collapses but
/// carries no candidate (mapping is paused until re-confirmed).
async fn collapse_linked_points(
    pool: &SqlitePool,
    user_id: i64,
    points: &mut Vec<ResumePoint>,
) -> Result<(), ProgressError> {
    use std::collections::HashSet;
    let uuids: HashSet<String> = points.iter().map(|p| p.record.book_uuid.clone()).collect();
    let mut drop_keys: HashSet<(String, ProgressFormat)> = HashSet::new();
    let mut linked_uuids: HashSet<String> = HashSet::new();
    for uuid in uuids {
        if crate::cross_format::get_link(pool, user_id, &uuid)
            .await
            .map_err(sqlx_of)?
            .is_none()
        {
            continue;
        }
        linked_uuids.insert(uuid.clone());
        let mut rows: Vec<&ResumePoint> = points
            .iter()
            .filter(|p| p.record.book_uuid == uuid)
            .collect();
        if rows.len() > 1 {
            // Deterministic ordering all the way down: same-second writes
            // tie-break on receipt time, then a fixed format rank (audio
            // first) — an unstable sort on equal keys would let the
            // surviving card flip between refreshes.
            rows.sort_by_key(|p| {
                std::cmp::Reverse((
                    p.record.client_updated_at,
                    p.record.updated_at,
                    matches!(p.record.format, ProgressFormat::Audio) as i64,
                ))
            });
            for older in &rows[1..] {
                drop_keys.insert((uuid.clone(), older.record.format));
            }
        }
    }
    points.retain(|p| !drop_keys.contains(&(p.record.book_uuid.clone(), p.record.format)));
    for p in points.iter_mut() {
        if !linked_uuids.contains(&p.record.book_uuid) {
            continue;
        }
        p.linked = true;
        let target = match p.record.format {
            ProgressFormat::Epub => ProgressFormat::Audio,
            ProgressFormat::Audio => ProgressFormat::Epub,
        };
        // A book that vanished mid-pass is routine here (resume_points
        // skips vanished books already) — only real DB errors propagate.
        let resume =
            match crate::cross_format::resume_candidate(pool, user_id, &p.record.book_uuid, target)
                .await
            {
                Ok(resume) => resume,
                Err(
                    crate::cross_format::CrossFormatError::BookNotFound
                    | crate::cross_format::CrossFormatError::AudioSetMismatch,
                ) => continue,
                Err(e) => return Err(sqlx_of(e)),
            };
        p.cross_format = resume.candidate;
    }
    Ok(())
}

/// Narrow a `CrossFormatError` into this module's error space — the two
/// crates share the same failure modes, and a vanished book mid-pass is
/// routine (treated as no link by the caller).
fn sqlx_of(e: crate::cross_format::CrossFormatError) -> ProgressError {
    match e {
        crate::cross_format::CrossFormatError::BookNotFound
        | crate::cross_format::CrossFormatError::AudioSetMismatch
        // Declaration-only refusals; the read paths this module calls
        // never produce them, but fold them safely rather than panic.
        | crate::cross_format::CrossFormatError::LinkRequired
        | crate::cross_format::CrossFormatError::CounterpartMissing => ProgressError::BookNotFound,
        crate::cross_format::CrossFormatError::Sqlx(inner) => ProgressError::Sqlx(inner),
    }
}

/// Which audio file a resume point plays, plus the duration and chapter
/// position measured against **that** file.
struct AudioTotals {
    book_file_id: i64,
    total_duration_seconds: f64,
    chapter_number: Option<i64>,
    chapter_count: Option<i64>,
}

/// Resolve the audio file for a progress row and measure duration + chapter
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
        chapter_number: chapter_number_at(&chapters, position),
        chapter_count: (!chapters.is_empty()).then_some(chapters.len() as i64),
    }))
}

/// 1-based chapter number at `elapsed` seconds, mirroring the player's
/// index-plus-one display (not the stored `file_chapters.ordinal`, which is
/// container-supplied and not guaranteed dense).
///
/// `pub(super)` rather than private: exercised directly by a boundary test
/// in `progress::tests` alongside the rest of the resume-card coverage.
pub(super) fn chapter_number_at(chapters: &[ChapterInfo], elapsed: f64) -> Option<i64> {
    if chapters.is_empty() {
        return None;
    }
    let idx = chapters
        .partition_point(|c| c.start_seconds <= elapsed)
        .saturating_sub(1);
    Some(idx as i64 + 1)
}
