//! Resume-candidate composition: the answer `GET
//! /api/books/{uuid}/cross-format-resume` serves. Gates on the link,
//! compares the two rows' ordering clocks, and maps the newer source onto
//! the target — anything unmappable degrades to `NothingNewer`.

use omnibus_shared::cross_format::{
    CrossFormatCandidate, CrossFormatResume, CrossFormatResumeState, MappingConfidence,
};
use omnibus_shared::ProgressFormat;
use sqlx::SqlitePool;

use crate::{progress, resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::anchors::{self, AnchorMap};
use super::link::get_link;
use super::mapping::{
    audio_fraction, audio_timeline, epub_source_fraction, link_is_stale, map_fraction_to_audio,
    AudioTimeline,
};
use super::{CrossFormatError, CrossFormatLink};

/// Everything both target arms read: the newer source row, the target row
/// the alignment gate compares it against, the timeline and anchor map the
/// mapping runs through, and the equivalence tolerance.
struct CandidateContext<'a> {
    target: ProgressFormat,
    source_format: ProgressFormat,
    source: &'a omnibus_shared::ProgressRecord,
    target_row: Option<&'a omnibus_shared::ProgressRecord>,
    timeline: &'a AudioTimeline,
    anchor_map: &'a Option<AnchorMap>,
    confidence: MappingConfidence,
    tol_frac: f64,
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

    let (anchor_map, confidence) = anchor_context(pool, book_id, &link, &timeline).await?;
    let follow = link.follow;
    // The gate only engages against an existing target position — skip the
    // stats query entirely on first-open targets.
    let tol_frac = match &target_row {
        Some(_) => equivalence_fraction(epub_total_chars(pool, book_id).await),
        None => equivalence_fraction(None),
    };
    let ctx = CandidateContext {
        target,
        source_format,
        source: &source,
        target_row: target_row.as_ref(),
        timeline: &timeline,
        anchor_map: &anchor_map,
        confidence,
        tol_frac,
    };
    let (candidate, aligned) = match target {
        ProgressFormat::Audio => audio_candidate(pool, book_id, &ctx).await,
        ProgressFormat::Epub => epub_candidate(pool, book_id, &ctx).await,
    };
    Ok(resume_state(candidate, aligned, follow))
}

/// Wrap a mapped candidate in the state prompt surfaces read.
fn resume_state(
    candidate: Option<CrossFormatCandidate>,
    aligned: bool,
    follow: bool,
) -> CrossFormatResume {
    match candidate {
        // Same spot within tolerance: the mapped position still rides
        // along (the hero's format-switch CTA wants it) but the state
        // tells prompt surfaces to stay quiet — a jump would move the
        // user nowhere, or worse, one quantization step backward.
        Some(candidate) if aligned => CrossFormatResume {
            state: CrossFormatResumeState::Aligned,
            candidate: Some(candidate),
            follow,
        },
        Some(candidate) => CrossFormatResume {
            state: CrossFormatResumeState::Candidate,
            candidate: Some(candidate),
            follow,
        },
        None => CrossFormatResume {
            state: CrossFormatResumeState::NothingNewer,
            candidate: None,
            follow,
        },
    }
}

/// The anchor pairs the mapping will run through, plus the confidence they
/// justify.
///
/// The anchored tier engages when the served EPUB has extracted structure
/// and its chapters match the audio marks; otherwise every path below
/// falls through to the linear tier. User-declared sync points outrank
/// both — they seed the anchor set and chapter anchors survive only where
/// they agree.
async fn anchor_context(
    pool: &SqlitePool,
    book_id: i64,
    link: &CrossFormatLink,
    timeline: &AudioTimeline,
) -> Result<(Option<AnchorMap>, MappingConfidence), CrossFormatError> {
    let chapter_map = match crate::book_file_with_id(pool, book_id, "EPUB").await? {
        Some((ebook_file_id, _)) => anchors::anchor_map(pool, ebook_file_id, timeline).await?,
        None => None,
    };
    let user_anchors: Vec<anchors::Anchor> = if timeline.total_seconds > 0.0 {
        link.user_anchors
            .iter()
            .map(|(t, s)| anchors::Anchor {
                text_frac: t.clamp(0.0, 1.0),
                audio_frac: (s / timeline.total_seconds).clamp(0.0, 1.0),
            })
            .collect()
    } else {
        Vec::new()
    };
    let confidence = if !user_anchors.is_empty() {
        MappingConfidence::UserAnchored
    } else if chapter_map.is_some() {
        MappingConfidence::ChapterAnchored
    } else {
        MappingConfidence::Linear
    };
    Ok((
        anchors::merge_user_anchors(&user_anchors, chapter_map),
        confidence,
    ))
}

/// Map a reading position onto the audio timeline. The bool is the
/// alignment gate: whether the mapped spot is the target's own spot within
/// tolerance.
async fn audio_candidate(
    pool: &SqlitePool,
    book_id: i64,
    ctx: &CandidateContext<'_>,
) -> (Option<CrossFormatCandidate>, bool) {
    let Some(src_frac) = epub_source_fraction(pool, book_id, ctx.source).await else {
        return (None, false);
    };
    let frac = match ctx.anchor_map {
        Some(map) => anchors::interpolate(&map.anchors, src_frac, true),
        None => src_frac,
    };
    let Some((file_id, seconds)) = map_fraction_to_audio(ctx.timeline, frac) else {
        return (None, false);
    };
    let timeline = ctx.timeline;
    let mapped_global = timeline
        .files
        .iter()
        .find(|f| f.book_file_id == file_id)
        .map(|f| f.start_seconds + seconds)
        .unwrap_or(frac * timeline.total_seconds);
    // The target's own spot on the same timeline. A file-less multi-file
    // row can't be placed, so the gate stands aside and the offer survives.
    let current_global = ctx.target_row.and_then(|t| {
        let secs = t.audio_position_seconds?;
        let file = t
            .book_file_id
            .or_else(|| (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id))?;
        audio_fraction(timeline, file, secs).map(|f| f * timeline.total_seconds)
    });
    let tol = (ctx.tol_frac * timeline.total_seconds)
        .max(audio_equivalence_floor(timeline.total_seconds));
    let aligned = current_global.is_some_and(|cur| (mapped_global - cur).abs() <= tol);
    let candidate = CrossFormatCandidate {
        target: ctx.target,
        source_format: ctx.source_format,
        source_client_updated_at: ctx.source.client_updated_at,
        confidence: ctx.confidence,
        book_file_id: Some(file_id),
        audio_position_seconds: Some(seconds),
        total_duration_seconds: Some(timeline.total_seconds),
        percent: None,
        fraction: None,
        epub_cfi: None,
        source_position_seconds: None,
        source_ahead: current_global.map(|cur| mapped_global > cur),
    };
    (Some(candidate), aligned)
}

/// Map a listening position onto the text. The bool is the alignment gate,
/// as in [`audio_candidate`].
async fn epub_candidate(
    pool: &SqlitePool,
    book_id: i64,
    ctx: &CandidateContext<'_>,
) -> (Option<CrossFormatCandidate>, bool) {
    let timeline = ctx.timeline;
    // The row's own file when recorded. A file-less row is only
    // unambiguous on a single-file timeline — guessing the first
    // of several would map another file's offset, so refuse.
    let mapped = ctx.source.audio_position_seconds.and_then(|seconds| {
        let file_id = ctx
            .source
            .book_file_id
            .or_else(|| (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id))?;
        audio_fraction(timeline, file_id, seconds)
    });
    let Some(raw_frac) = mapped else {
        return (None, false);
    };
    let frac = match ctx.anchor_map {
        Some(map) => anchors::interpolate(&map.anchors, raw_frac, false),
        None => raw_frac,
    };
    let pct = ((100.0 * frac).floor() as i64).clamp(0, 100);
    let current = match ctx.target_row {
        Some(t) => epub_source_fraction(pool, book_id, t).await,
        None => None,
    };
    let aligned = current.is_some_and(|cur| (frac - cur).abs() <= ctx.tol_frac);
    let candidate = CrossFormatCandidate {
        target: ctx.target,
        source_format: ctx.source_format,
        source_client_updated_at: ctx.source.client_updated_at,
        confidence: ctx.confidence,
        book_file_id: None,
        audio_position_seconds: None,
        total_duration_seconds: None,
        percent: Some(pct),
        fraction: Some(frac.clamp(0.0, 1.0)),
        // Derived on request by the resume endpoint
        // (`derive_candidate_cfi`), never here — every
        // hero card calls this fn and must stay I/O-free.
        epub_cfi: None,
        source_position_seconds: Some(raw_frac * timeline.total_seconds),
        source_ahead: current.map(|cur| frac > cur),
    };
    (Some(candidate), aligned)
}

/// Positions closer than this fraction of the whole book count as "the
/// same spot" — an offer inside it is quantization noise, not signal.
/// Base 0.5%, widened to two reader locations (~1024 visible chars each,
/// the epub.js jump granularity) on short books where one location is a
/// large slice, and capped at 5% so a tiny book can't suppress every
/// offer outright.
pub(super) fn equivalence_fraction(total_chars: Option<i64>) -> f64 {
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
pub(super) fn audio_equivalence_floor(total_seconds: f64) -> f64 {
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
