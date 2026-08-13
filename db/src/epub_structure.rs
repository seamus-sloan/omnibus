//! Storage for the derived EPUB structure tables (`epub_spine_stats`,
//! `ebook_chapters`): bulk replace at extraction time, reads for the
//! cross-format mapper, and O(1) percent arithmetic over stored stats.
//! Both tables cascade from `book_files`, so removal and the Changed
//! sync path clear them without help.

use sqlx::SqlitePool;

use crate::ebook::toc::EpubStructure;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum EpubStructureError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// One stored spine-stat row, in spine order.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SpineStatRow {
    pub spine_index: i64,
    pub href: String,
    pub visible_chars: i64,
    pub chars_before: i64,
}

/// One stored chapter row, in TOC order.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct EbookChapterRow {
    pub ordinal: i64,
    pub title: String,
    pub href: String,
    pub spine_index: i64,
    pub start_chars: i64,
}

/// Replace both structure tables for one `book_files` row atomically.
/// Idempotent: re-running with the same extraction converges.
pub async fn replace_structure(
    pool: &SqlitePool,
    book_file_id: i64,
    structure: &EpubStructure,
) -> Result<(), EpubStructureError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM epub_spine_stats WHERE book_file_id = ?")
        .bind(book_file_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM ebook_chapters WHERE book_file_id = ?")
        .bind(book_file_id)
        .execute(&mut *tx)
        .await?;

    // Chunked VALUES inserts under SQLite's 999-bind cap: 5 binds per
    // spine row → 199/chunk, 6 per chapter row → 166/chunk (the
    // `bulk_insert_chapters` pattern).
    let mut chars_before: i64 = 0;
    let spine_rows: Vec<(i64, String, i64, i64)> = structure
        .spine
        .iter()
        .map(|s| {
            let row = (s.spine_index, s.href.clone(), s.visible_chars, chars_before);
            chars_before += s.visible_chars;
            row
        })
        .collect();
    for chunk in spine_rows.chunks(199) {
        let placeholders = vec!["(?, ?, ?, ?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO epub_spine_stats
                (book_file_id, spine_index, href, visible_chars, chars_before)
             VALUES {placeholders}"
        );
        let mut q = sqlx::query(&sql);
        for (spine_index, href, visible_chars, before) in chunk {
            q = q
                .bind(book_file_id)
                .bind(spine_index)
                .bind(href)
                .bind(visible_chars)
                .bind(before);
        }
        q.execute(&mut *tx).await?;
    }
    for chunk in structure.chapters.chunks(166) {
        let placeholders = vec!["(?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO ebook_chapters
                (book_file_id, ordinal, title, href, spine_index, start_chars)
             VALUES {placeholders}"
        );
        let mut q = sqlx::query(&sql);
        for c in chunk {
            q = q
                .bind(book_file_id)
                .bind(c.ordinal)
                .bind(&c.title)
                .bind(&c.href)
                .bind(c.spine_index)
                .bind(c.start_chars);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Stored spine stats for one file, spine order. Empty when never extracted.
pub async fn get_spine_stats(
    pool: &SqlitePool,
    book_file_id: i64,
) -> Result<Vec<SpineStatRow>, EpubStructureError> {
    Ok(sqlx::query_as(
        "SELECT spine_index, href, visible_chars, chars_before
         FROM epub_spine_stats WHERE book_file_id = ? ORDER BY spine_index",
    )
    .bind(book_file_id)
    .fetch_all(pool)
    .await?)
}

/// Stored chapters for one file, TOC order. Empty when never extracted —
/// or extracted from a book that honestly has no TOC; `get_spine_stats`
/// being non-empty is what distinguishes the two.
pub async fn get_chapters(
    pool: &SqlitePool,
    book_file_id: i64,
) -> Result<Vec<EbookChapterRow>, EpubStructureError> {
    Ok(sqlx::query_as(
        "SELECT ordinal, title, href, spine_index, start_chars
         FROM ebook_chapters WHERE book_file_id = ? ORDER BY ordinal",
    )
    .bind(book_file_id)
    .fetch_all(pool)
    .await?)
}

/// Whole-book visible-text fraction (`0.0..=1.0`) for a position, from
/// stored stats alone. The full-precision half of [`percent_at`], exposed
/// for the cross-format mapping where integer-percent quantization would
/// cost up to 1% of a book (~30 minutes of a long audiobook). `None` when
/// the stats don't cover `spine_index`.
pub fn fraction_at(stats: &[SpineStatRow], spine_index: i64, offset_in_file: u64) -> Option<f64> {
    let row = stats.iter().find(|s| s.spine_index == spine_index)?;
    // Order-independent total (never trusts slice order) and saturating
    // arithmetic throughout: a corrupt or absurd offset clamps rather
    // than overflowing.
    let total: i64 = stats
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
    if total <= 0 {
        return Some(0.0);
    }
    let at = row
        .chars_before
        .max(0)
        .saturating_add(i64::try_from(offset_in_file).unwrap_or(i64::MAX));
    Some((at as f64 / total as f64).clamp(0.0, 1.0))
}

/// Whole-book visible-text percent for a position, from stored stats alone:
/// floor semantics and clamping identical to `kobo_position`'s
/// `book_percent`, so the two derivation paths cannot disagree. `None`
/// when the stats don't cover `spine_index`; a book that measured empty
/// reports 0.
pub fn percent_at(stats: &[SpineStatRow], spine_index: i64, offset_in_file: u64) -> Option<i64> {
    fraction_at(stats, spine_index, offset_in_file)
        .map(|f| ((100.0 * f).floor() as i64).clamp(0, 100))
}
