//! Place a stored anchor string — a highlight's range CFI, a bookmark's CFI
//! or audio timestamp — in the book, so a caller never repeats the same
//! spine arithmetic per annotation.
//!
//! Resolution is **spine-granular** and reads nothing but the structure
//! tables: a CFI carries its spine step, `epub_spine_stats` turns that into
//! a whole-book percent, and `ebook_chapters` names the chapter. No EPUB is
//! opened, so placing sixteen highlights costs the same three queries as
//! placing one — which is the point, since the alternative was sixteen
//! round-trips of client-side CFI arithmetic.

use sqlx::SqlitePool;

use crate::epub_structure::{self, EbookChapterRow, SpineStatRow};

#[cfg(test)]
mod tests;

/// How an annotation list is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AnnotationOrder {
    /// By where the anchor sits in the book. The default: a highlight list
    /// is read as a pass through the text, and unplaceable anchors sort
    /// last so they never interrupt that run.
    #[default]
    Position,
    /// By creation time, oldest first — the order these lists had before
    /// they could be placed.
    Chronological,
}

impl AnnotationOrder {
    /// Parse the wire token. Anything unrecognised is the default, so a
    /// typo yields the useful order rather than an error.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "chronological" | "created" => Self::Chronological,
            _ => Self::Position,
        }
    }
}

/// Sort key placing unplaceable anchors last: `Option`'s own ordering puts
/// `None` first, which would open every list with the anchors it could say
/// least about.
pub fn position_key(spine_index: Option<i64>, created_at: i64, id: i64) -> (bool, i64, i64, i64) {
    (
        spine_index.is_none(),
        spine_index.unwrap_or(i64::MAX),
        created_at,
        id,
    )
}

/// Failure space for the anchor placement reads.
#[derive(Debug, thiserror::Error)]
pub enum AnchorError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<epub_structure::EpubStructureError> for AnchorError {
    fn from(e: epub_structure::EpubStructureError) -> Self {
        match e {
            epub_structure::EpubStructureError::Sqlx(inner) => AnchorError::Sqlx(inner),
        }
    }
}

/// Where one anchor sits. Every field is optional: an anchor the structure
/// cannot place reports nothing rather than a guess.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnchorPlacement {
    pub spine_index: Option<i64>,
    pub chapter_title: Option<String>,
    pub percent_through_book: Option<f64>,
}

/// One book's structure, loaded once and reused across its annotations.
pub struct AnchorIndex {
    spine: Vec<SpineStatRow>,
    chapters: Vec<EbookChapterRow>,
    total_chars: i64,
    /// Whole-book audio runtime, for bookmarks stored as a timestamp.
    audio_seconds: Option<f64>,
}

impl AnchorIndex {
    /// Load the structure for one book. An unknown uuid, a book with no
    /// EPUB, or one whose structure has never been extracted all yield an
    /// index that places nothing — callers get empty placements, not errors.
    pub async fn load(pool: &SqlitePool, book_uuid: &str) -> Result<Self, AnchorError> {
        let mut index = Self {
            spine: Vec::new(),
            chapters: Vec::new(),
            total_chars: 0,
            audio_seconds: None,
        };
        let Some(canonical) = crate::resolve_canonical_book_uuid(pool, book_uuid)
            .await
            .map_err(books_error)?
        else {
            return Ok(index);
        };
        index.audio_seconds = audio_runtime(pool, &canonical).await?;

        let Some(book_id) = crate::resolve_book_id_by_uuid(pool, &canonical)
            .await
            .map_err(books_error)?
        else {
            return Ok(index);
        };
        let Some((file_id, _)) = crate::book_file_with_id(pool, book_id, "EPUB")
            .await
            .map_err(books_error)?
        else {
            return Ok(index);
        };
        index.spine = epub_structure::get_spine_stats(pool, file_id).await?;
        index.chapters = epub_structure::get_chapters(pool, file_id).await?;
        index.total_chars = index
            .spine
            .iter()
            .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
        Ok(index)
    }

    /// Place one anchor: an `epubcfi(…)` point or range, or a bare number of
    /// seconds (an audiobook bookmark).
    pub fn locate(&self, anchor: &str) -> AnchorPlacement {
        if let Some(spine_index) = spine_index_of(anchor) {
            return self.locate_spine(spine_index);
        }
        // A bookmark's `position` is an opaque token: seconds for the
        // player, a CFI for the reader. Only a bare number can be the
        // former, and `parse` is what tells them apart without guessing.
        if let (Ok(seconds), Some(total)) = (anchor.trim().parse::<f64>(), self.audio_seconds) {
            if seconds.is_finite() && seconds >= 0.0 && total > 0.0 {
                return AnchorPlacement {
                    percent_through_book: Some((seconds / total * 100.0).clamp(0.0, 100.0)),
                    ..AnchorPlacement::default()
                };
            }
        }
        AnchorPlacement::default()
    }

    /// Place a spine index against the stored structure.
    fn locate_spine(&self, spine_index: i64) -> AnchorPlacement {
        let Some(row) = self.spine.iter().find(|s| s.spine_index == spine_index) else {
            // The CFI named a spine step, which is worth reporting even when
            // no stats exist to measure it against.
            return AnchorPlacement {
                spine_index: Some(spine_index),
                ..AnchorPlacement::default()
            };
        };
        let percent = (self.total_chars > 0).then(|| {
            (row.chars_before.max(0) as f64 / self.total_chars as f64 * 100.0).clamp(0.0, 100.0)
        });
        AnchorPlacement {
            spine_index: Some(spine_index),
            chapter_title: self
                .chapters
                .iter()
                .filter(|c| c.spine_index <= spine_index)
                .max_by_key(|c| (c.spine_index, c.ordinal))
                .map(|c| c.title.clone()),
            percent_through_book: percent,
        }
    }
}

/// The spine step a CFI names, whether it is a point or a range anchor.
/// `None` for anything that is not a CFI at all.
fn spine_index_of(anchor: &str) -> Option<i64> {
    crate::kobo_position::cfi::parse_cfi(anchor)
        .map(|c| c.spine_index)
        .or_else(|| crate::kobo_position::cfi::parse_range_cfi(anchor).map(|r| r.spine_index))
        .and_then(|idx| i64::try_from(idx).ok())
}

/// Whole-book audio runtime for a book, summed over the first audio file's
/// parts. `None` when the book has no audio.
async fn audio_runtime(pool: &SqlitePool, book_uuid: &str) -> Result<Option<f64>, AnchorError> {
    let Some(resolved) = crate::hls::resolve_audiobook(pool, book_uuid)
        .await
        .map_err(|e| match e {
            crate::hls::HlsError::Db(inner) => AnchorError::Sqlx(inner),
        })?
    else {
        return Ok(None);
    };
    let parts = crate::hls::get_parts(pool, resolved.book_file_id)
        .await
        .map_err(|e| match e {
            crate::hls::HlsError::Db(inner) => AnchorError::Sqlx(inner),
        })?;
    let total: f64 = parts.iter().map(|p| p.duration_seconds).sum();
    Ok((total > 0.0).then_some(total))
}

/// Narrow a `BooksError` into this module's error space.
fn books_error(e: crate::books::BooksError) -> AnchorError {
    match e {
        crate::books::BooksError::Db(inner) => AnchorError::Sqlx(inner),
        crate::books::BooksError::OverridesJson(inner) => {
            AnchorError::Sqlx(sqlx::Error::Decode(Box::new(inner)))
        }
        crate::books::BooksError::Other(msg) => AnchorError::Sqlx(sqlx::Error::Decode(msg.into())),
    }
}
