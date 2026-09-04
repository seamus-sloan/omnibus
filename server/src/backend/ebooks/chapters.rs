//! `/api/ebooks/{uuid}/chapters*` handlers: the chapter listing and the
//! bounded plain-text chapter read. Ordinary [`AuthUser`]-gated JSON reads —
//! derived text, not byte-serving, so none of the `serve_file` validator
//! machinery applies. A book whose served format has no extractable text
//! (comic-only, audiobook-only) answers `has_text: false` rather than a
//! 404, which stays reserved for an unknown uuid.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, ebook::text::extract_chapter_text, epub_structure};
use omnibus_shared::{
    ChapterListEntry, ChapterListResponse, ChapterTextResponse, CHAPTER_TEXT_MAX_CHARS,
};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::backend::{internal, AppState};

/// Query params for the chapter text read: a char offset to resume from and
/// an optional smaller slice size (clamped to
/// [`CHAPTER_TEXT_MAX_CHARS`] — the cap holds regardless of what the client
/// asks for).
#[derive(Deserialize)]
pub(crate) struct ChapterTextQuery {
    offset: Option<usize>,
    limit: Option<usize>,
    /// Cut the slice off at the caller's own furthest recorded position in
    /// this book, so a reader mid-book cannot be handed text they have not
    /// reached.
    #[serde(default)]
    stop_at_progress: bool,
}

/// The `has_text: false` answer both endpoints share for a book with no
/// EPUB to read.
fn no_text_list(uuid: &str) -> Response {
    Json(ChapterListResponse {
        book_uuid: uuid.to_string(),
        has_text: false,
        spine_count: 0,
        chapters: Vec::new(),
    })
    .into_response()
}

/// Chapter listing: TOC titles plus the spine index each text read is
/// addressed by, from the persisted structure tables. A book scanned but
/// not yet backfilled falls back to extracting the structure from the file
/// on the fly (read-only — the backfill remains the writer).
pub(crate) async fn get_ebook_chapters(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    let (file_id, path) = match db::book_file_with_id(&state.pool, id, "EPUB").await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => return no_text_list(&uuid),
        Err(e) => return internal("book_file_with_id", e),
    };

    let stats = match epub_structure::get_spine_stats(&state.pool, file_id).await {
        Ok(s) => s,
        Err(e) => return internal("get_spine_stats", e),
    };
    let (spine_count, chapters) = if stats.is_empty() {
        // Never extracted (the post-scan backfill hasn't reached it yet):
        // derive the same structure straight from the file.
        let extracted =
            tokio::task::spawn_blocking(move || db::ebook::toc::extract_structure_from_path(&path))
                .await;
        let structure = match extracted {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return internal("extract_structure_from_path", e),
            Err(e) => return internal("extract_structure_from_path", e),
        };
        // An archive that opened but yielded no readable spine has no text
        // to address — the same honest answer a non-EPUB book gets.
        let Some(structure) = structure else {
            return no_text_list(&uuid);
        };
        let chapters = structure
            .chapters
            .iter()
            .map(|c| ChapterListEntry {
                ordinal: c.ordinal,
                title: c.title.clone(),
                spine_index: c.spine_index,
            })
            .collect();
        (structure.spine.len() as i64, chapters)
    } else {
        let rows = match epub_structure::get_chapters(&state.pool, file_id).await {
            Ok(rows) => rows,
            Err(e) => return internal("get_chapters", e),
        };
        let chapters = rows
            .iter()
            .map(|c| ChapterListEntry {
                ordinal: c.ordinal,
                title: c.title.clone(),
                spine_index: c.spine_index,
            })
            .collect();
        (stats.len() as i64, chapters)
    };

    Json(ChapterListResponse {
        book_uuid: uuid,
        has_text: true,
        spine_count,
        chapters,
    })
    .into_response()
}

/// Bounded plain-text read of one spine document. 404 covers an unknown
/// uuid and an out-of-range spine index; a book with no EPUB answers
/// `has_text: false`; an unreadable archive is a 500. The slice is char-
/// addressed (`?offset=`, `?limit=`) and capped at
/// [`CHAPTER_TEXT_MAX_CHARS`], with `truncated` / `next_offset` reporting
/// the boundary.
pub(crate) async fn get_ebook_chapter_text(
    user: AuthUser,
    State(state): State<AppState>,
    Path((uuid, spine_index)): Path<(String, usize)>,
    Query(q): Query<ChapterTextQuery>,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    let path = match db::book_file_path(&state.pool, id, "EPUB").await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Json(ChapterTextResponse {
                book_uuid: uuid,
                has_text: false,
                spine_index: spine_index as i64,
                text: String::new(),
                offset: 0,
                total_chars: 0,
                truncated: false,
                next_offset: None,
                truncated_by_progress: false,
            })
            .into_response()
        }
        Err(e) => return internal("book_file_path", e),
    };

    let extracted =
        tokio::task::spawn_blocking(move || extract_chapter_text(&path, spine_index)).await;
    let text = match extracted {
        Ok(Ok(Some(text))) => text,
        Ok(Ok(None)) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => return internal("extract_chapter_text", e),
        Err(e) => return internal("extract_chapter_text", e),
    };

    let stop_at = if q.stop_at_progress {
        match progress_cutoff(&state, user.id, &uuid, id, spine_index, text.chars().count()).await {
            Ok(cutoff) => cutoff,
            Err(e) => return internal("progress_cutoff", e),
        }
    } else {
        None
    };

    Json(slice_text(uuid, spine_index, &text, q, stop_at)).into_response()
}

/// How far into this chapter the caller has read, in chars — the ceiling
/// `?stop_at_progress=true` slices at. `None` means no ceiling applies.
///
/// Three outcomes, and the difference matters:
/// - the reader is **past** this chapter → no ceiling (`None`),
/// - the reader is **inside** it → the char offset they reached,
/// - the reader is **before** it, or their position cannot be placed at all
///   → `Some(0)`, i.e. withhold the whole chapter.
///
/// That last case is the conservative one on purpose: a caller asking not to
/// be shown text past the reader must not be handed a whole chapter because
/// the server could not work out where they were.
async fn progress_cutoff(
    state: &AppState,
    user_id: i64,
    uuid: &str,
    book_id: i64,
    spine_index: usize,
    chapter_chars: usize,
) -> Result<Option<usize>, anyhow::Error> {
    let progress = db::progress::book_progress(&state.pool, user_id, uuid, None).await?;
    let Some(percent) = progress
        .records
        .iter()
        .filter_map(|d| d.resolved.percent_through_book)
        .max_by(f64::total_cmp)
    else {
        // No placeable position anywhere in the book: withhold everything.
        return Ok(Some(0));
    };
    let Some((file_id, _)) = db::book_file_with_id(&state.pool, book_id, "EPUB").await? else {
        return Ok(Some(0));
    };
    let stats = epub_structure::get_spine_stats(&state.pool, file_id).await?;
    let Some((reader_spine, reader_offset)) =
        epub_structure::position_at_fraction(&stats, percent / 100.0)
    else {
        return Ok(Some(0));
    };
    let spine_index = spine_index as i64;
    Ok(match reader_spine.cmp(&spine_index) {
        std::cmp::Ordering::Greater => None,
        std::cmp::Ordering::Equal => Some((reader_offset as usize).min(chapter_chars)),
        std::cmp::Ordering::Less => Some(0),
    })
}

/// Cut the requested char window out of the full extracted text and report
/// the boundary. An `offset` at or past the end yields an empty,
/// non-truncated slice rather than an error — it is how a paginating client
/// naturally terminates.
fn slice_text(
    book_uuid: String,
    spine_index: usize,
    text: &str,
    q: ChapterTextQuery,
    stop_at: Option<usize>,
) -> ChapterTextResponse {
    let total_chars = text.chars().count();
    let offset = q.offset.unwrap_or(0).min(total_chars);
    let limit = q
        .limit
        .unwrap_or(CHAPTER_TEXT_MAX_CHARS)
        .clamp(1, CHAPTER_TEXT_MAX_CHARS);
    let ceiling = stop_at.unwrap_or(total_chars).min(total_chars);
    let take = limit.min(ceiling.saturating_sub(offset));
    let slice: String = text.chars().skip(offset).take(take).collect();
    let end = offset + slice.chars().count();
    // A slice stopped at the reader's position is not "truncated": that flag
    // invites a caller to page onward, which is the one thing this must not
    // let it do. The separate flag says the cut was deliberate and final.
    let truncated_by_progress = ceiling < total_chars && end >= ceiling;
    let truncated = end < total_chars && !truncated_by_progress;
    ChapterTextResponse {
        book_uuid,
        has_text: true,
        spine_index: spine_index as i64,
        text: slice,
        offset: offset as i64,
        total_chars: total_chars as i64,
        truncated,
        next_offset: truncated.then_some(end as i64),
        truncated_by_progress,
    }
}
