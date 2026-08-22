//! The alignment modal's read: link + staleness, the anchor preview, both
//! lanes (ebook chapter ticks, per-file audio segments), and the two
//! current positions — assembled in one call.

use omnibus_shared::cross_format::{
    AlignmentAudioFile, AlignmentAudioPosition, AlignmentEbook, AlignmentEbookChapter,
    AlignmentLink, AlignmentMatch, AlignmentPosition, AlignmentView, CrossFormatLinkMode,
    MappingConfidence,
};
use omnibus_shared::ProgressFormat;
use sqlx::SqlitePool;

use crate::{progress, resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::anchors;
use super::link::get_link;
use super::mapping::{audio_files, audio_timeline, link_is_stale, AudioTimeline};
use super::{CrossFormatError, CrossFormatLink};

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
    let link = link_summary(pool, book_id, &raw_link).await?;
    let stale = link.as_ref().is_some_and(|l| l.stale);
    let preview = anchor_preview(pool, book_id, &raw_link, stale).await?;
    let ebook = ebook_lane(pool, book_id).await?;
    let audio = audio_lane(pool, book_id).await?;
    let (reading, listening) = current_positions(pool, user_id, &book_uuid).await?;

    Ok(AlignmentView {
        link,
        anchor_match: preview.anchor_match,
        ebook,
        audio_files: audio,
        reading,
        listening,
        anchor_pairs: preview.anchor_pairs,
        audio_chapter_marks: preview.audio_chapter_marks,
    })
}

/// The served link record plus the staleness check the modal keys its
/// re-confirm copy on.
async fn link_summary(
    pool: &SqlitePool,
    book_id: i64,
    raw_link: &Option<CrossFormatLink>,
) -> Result<Option<AlignmentLink>, CrossFormatError> {
    let Some(link) = raw_link else {
        return Ok(None);
    };
    let stale = link_is_stale(pool, book_id, link).await?;
    Ok(Some(AlignmentLink {
        mode: link.mode,
        primary_book_file_id: link.primary_book_file_id,
        stale,
        confirmed_at: link.confirmed_at,
        follow: link.follow,
        user_anchors: link.user_anchors.len() as i64,
    }))
}

/// How the mapping would resolve today: the match statistics, the pairs
/// the preview threads are drawn from, and the usable audio mark count.
#[derive(Default)]
struct AnchorPreview {
    anchor_match: Option<AlignmentMatch>,
    anchor_pairs: Vec<(f64, f64)>,
    audio_chapter_marks: i64,
}

/// Evaluate the anchor preview. For an unlinked book this runs against the
/// default sequence declaration so the modal can honestly say whether
/// anchoring would engage; a stale link stays quiet (mapping is paused
/// anyway).
async fn anchor_preview(
    pool: &SqlitePool,
    book_id: i64,
    raw_link: &Option<CrossFormatLink>,
    stale: bool,
) -> Result<AnchorPreview, CrossFormatError> {
    if stale {
        return Ok(AnchorPreview::default());
    }
    let preview = raw_link.clone().unwrap_or(CrossFormatLink {
        mode: CrossFormatLinkMode::Sequence,
        primary_book_file_id: None,
        audio_snapshot: String::new(),
        confirmed_at: 0,
        follow: false,
        user_anchors: Vec::new(),
    });
    let Some(timeline) = audio_timeline(pool, book_id, &preview).await? else {
        return Ok(AnchorPreview::default());
    };
    // Computed once and threaded into `anchor_map_from_marks` below, rather
    // than letting `anchor_map` recompute the same batched fetch.
    let audio = anchors::audio_marks(pool, &timeline).await?;
    let mut out = AnchorPreview {
        audio_chapter_marks: anchors::usable_audio_mark_count(&audio),
        ..AnchorPreview::default()
    };
    let Some((ebook_file_id, _)) = crate::book_file_with_id(pool, book_id, "EPUB").await? else {
        return Ok(out);
    };
    // User anchors fold in exactly as the jump path does, so the preview's
    // threads and interpolation are the pairs the mapping will really use.
    let chapter_map =
        anchors::anchor_map_from_marks(pool, ebook_file_id, &timeline, &audio).await?;
    let user = preview_user_anchors(raw_link, &timeline);
    let stats = chapter_map.as_ref().map(|m| (m.matched, m.ebook_chapters));
    if let Some(merged) = anchors::merge_user_anchors(&user, chapter_map) {
        out.anchor_pairs = merged
            .anchors
            .iter()
            .map(|a| (a.text_frac, a.audio_frac))
            .collect();
    }
    out.anchor_match = stats.map(|(matched, ebook_chapters)| AlignmentMatch {
        matched,
        ebook_chapters,
        confidence: MappingConfidence::ChapterAnchored,
    });
    Ok(out)
}

/// The link's stored "synced here" pairs as timeline fractions.
fn preview_user_anchors(
    raw_link: &Option<CrossFormatLink>,
    timeline: &AudioTimeline,
) -> Vec<anchors::Anchor> {
    raw_link
        .as_ref()
        .filter(|_| timeline.total_seconds > 0.0)
        .map(|l| {
            l.user_anchors
                .iter()
                .map(|(t, s)| anchors::Anchor {
                    text_frac: t.clamp(0.0, 1.0),
                    audio_frac: (s / timeline.total_seconds).clamp(0.0, 1.0),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Text lane: stored spine stats (0071). Absent until the structure
/// backfill reaches the book — the modal shows the honest linear notice in
/// that case rather than fabricated ticks.
async fn ebook_lane(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<AlignmentEbook>, CrossFormatError> {
    let Some((file_id, _)) = crate::book_file_with_id(pool, book_id, "EPUB").await? else {
        return Ok(None);
    };
    let stats = crate::epub_structure::get_spine_stats(pool, file_id).await?;
    let total: i64 = stats
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
    if stats.is_empty() || total <= 0 {
        return Ok(None);
    }
    let chapters = crate::epub_structure::get_chapters(pool, file_id)
        .await?
        .into_iter()
        .map(|c| AlignmentEbookChapter {
            title: c.title,
            percent: (100.0 * c.start_chars.max(0) as f64 / total as f64).clamp(0.0, 100.0),
        })
        .collect();
    Ok(Some(AlignmentEbook {
        total_chars: total,
        chapters,
    }))
}

/// Audio lane: every audio file in ordinal order with its chapter ticks.
async fn audio_lane(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Vec<AlignmentAudioFile>, CrossFormatError> {
    let files = audio_files(pool, book_id).await?;
    let ids: Vec<i64> = files.iter().map(|f| f.book_file_id).collect();
    let mut starts_by_file = bulk_chapter_starts(pool, &ids).await?;
    Ok(files
        .into_iter()
        .map(|f| AlignmentAudioFile {
            book_file_id: f.book_file_id,
            label: f.scan_key,
            duration_seconds: f.duration_seconds,
            chapter_starts: starts_by_file.remove(&f.book_file_id).unwrap_or_default(),
        })
        .collect())
}

/// The two current positions the lanes place their chips at.
async fn current_positions(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<(Option<AlignmentPosition>, Option<AlignmentAudioPosition>), CrossFormatError> {
    let reading = progress::get_progress(pool, user_id, book_uuid, ProgressFormat::Epub)
        .await?
        .map(|r| AlignmentPosition {
            percent: r.progress_percent,
            client_updated_at: r.client_updated_at,
        });
    let listening = progress::get_progress(pool, user_id, book_uuid, ProgressFormat::Audio)
        .await?
        .and_then(|r| {
            r.audio_position_seconds
                .map(|seconds| AlignmentAudioPosition {
                    book_file_id: r.book_file_id,
                    seconds,
                    client_updated_at: r.client_updated_at,
                })
        });
    Ok((reading, listening))
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
