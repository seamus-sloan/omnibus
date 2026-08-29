//! Full-text index over EPUB chapter text (migration `0087`): the post-scan
//! worker pass that populates `book_content_chapters` / `book_content_fts`,
//! and the bm25-ranked content-search read path. Populated by
//! `worker::Task::BackfillContentFts` after each ebook scan; read by the
//! `/api/search/content` REST handler.

mod extract;

use std::path::PathBuf;

use omnibus_shared::ContentSearchHit;
use sqlx::{Row, SqlitePool};

use crate::helpers::{cap_query_len, library_paths_json, sanitize_fts_query, visible_book_sql};

pub use extract::extract_chapter_texts;

/// Cap on returned content hits — one chapter-level citation list, not a
/// paginated browse surface.
const MAX_CONTENT_HITS: i64 = 50;

/// Content-search failure space: the read path touches nothing but the DB,
/// so one transparent variant keeps `sqlx::Error` from crossing the module
/// boundary (rule 02).
#[derive(Debug, thiserror::Error)]
pub enum ContentFtsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Search `book_content_fts` across every configured library path, best
/// bm25 rank first, capped at [`MAX_CONTENT_HITS`].
///
/// `q` runs through the same length cap and token sanitisation as the
/// metadata search ([`sanitize_fts_query`] — no column facets here, the
/// index has one column), so arbitrary user input is safe to pass through;
/// an empty or unusable query yields an empty vec. Each hit cites
/// `(book_uuid, spine_index)` plus a `snippet()` excerpt with matched terms
/// wrapped in `[`…`]`.
pub async fn search_content_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
) -> Result<Vec<ContentSearchHit>, ContentFtsError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let capped = cap_query_len(q);
    let Some(match_expr) = sanitize_fts_query(&capped) else {
        return Ok(Vec::new());
    };
    let visible = visible_book_sql("b", "l", "?");
    let sql = format!(
        r"
        SELECT c.book_uuid,
               c.spine_index,
               COALESCE(NULLIF(b.title, ''), b.scan_key) AS title,
               snippet(book_content_fts, 0, '[', ']', '…', 12) AS snip
        FROM book_content_fts
        JOIN book_content_chapters c ON c.id = book_content_fts.rowid
        JOIN books b ON b.uuid = c.book_uuid
        JOIN scan_roots l ON l.id = b.library_id
        WHERE book_content_fts MATCH ?
          AND {visible}
        ORDER BY bm25(book_content_fts), c.book_uuid, c.spine_index
        LIMIT ?
        "
    );
    let rows = sqlx::query(&sql)
        .bind(&match_expr)
        .bind(library_paths_json(library_paths))
        .bind(MAX_CONTENT_HITS)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ContentSearchHit {
            book_uuid: r.get("book_uuid"),
            spine_index: r.get("spine_index"),
            title: r.get("title"),
            snippet: r.get("snip"),
        })
        .collect())
}

/// A book whose content index is missing or stale: the lowest-ordinal EPUB
/// file's current stat differs from the stored snapshot (or no rows exist —
/// a freshly Added book looks identical to a stale one here, which is the
/// point: rule 09's derived-validator principle, one statement for both).
struct Candidate {
    book_uuid: String,
    title: String,
    path: PathBuf,
    mtime_epoch: i64,
    size_bytes: i64,
}

/// Every EPUB book under `library_path` needing (re)extraction. The compared
/// file is the book's lowest-ordinal EPUB — the one `book_file_path` serves
/// and the one a reader gets, matching rule 09's "the compared file is the
/// one the server would serve". Non-EPUB books never join and are skipped
/// silently (audiobooks and comics have no extractable text).
async fn fetch_candidates(pool: &SqlitePool, library_path: &str) -> anyhow::Result<Vec<Candidate>> {
    /// `(uuid, title, library root, dir, stem, format, mtime_epoch, size_bytes)`.
    type CandidateRow = (String, String, String, String, String, String, i64, i64);
    let rows: Vec<CandidateRow> = sqlx::query_as(
        "SELECT b.uuid, COALESCE(NULLIF(b.title, ''), b.scan_key), \
                COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format, bf.mtime_epoch, bf.size_bytes \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_files bf ON bf.id = ( \
             SELECT id FROM book_files \
             WHERE book_id = b.id AND format = 'EPUB' COLLATE NOCASE \
             ORDER BY ordinal LIMIT 1) \
         WHERE l.path = ? \
           AND NOT EXISTS (SELECT 1 FROM book_content_chapters c \
                           WHERE c.book_uuid = b.uuid \
                             AND c.mtime_epoch = bf.mtime_epoch \
                             AND c.size_bytes = bf.size_bytes) \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(book_uuid, title, lib, dir, stem, fmt, mtime_epoch, size_bytes)| Candidate {
                book_uuid,
                title,
                path: std::path::Path::new(&lib)
                    .join(&dir)
                    .join(format!("{stem}.{}", fmt.to_lowercase())),
                mtime_epoch,
                size_bytes,
            },
        )
        .collect())
}

/// (Re)index chapter text for every EPUB book under `library_path` whose
/// stored snapshot no longer matches the served file, and prune rows whose
/// uuid no longer resolves to any book (deleted, or merged away — the
/// cascade-free half of the migration's soft-reference choice).
///
/// A Changed file replaces the whole book's rows (delete + reinsert in one
/// transaction), so a shrunk edition can't leave phantom tail chapters. An
/// unreadable file is logged and skipped — retried next scan, mirroring the
/// sibling backfills — and a readable book whose every chapter is navigation
/// or empty stores no rows, so it re-extracts each scan at the cost of one
/// zip open (the `backfill_covers` retry semantics). `on_progress(processed,
/// total, item)` mirrors the sibling backfills.
pub async fn backfill_content_fts(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    // Prune first so a candidate re-index can't resurrect an orphan's rows'
    // uniqueness slots, and so the FTS index stops serving deleted books
    // even on a pass with no candidates.
    sqlx::query(
        "DELETE FROM book_content_chapters WHERE book_uuid NOT IN (SELECT uuid FROM books)",
    )
    .execute(pool)
    .await?;

    let candidates = fetch_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }
    let total = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    tracing::info!(count = total, "indexing epub content for search");

    let mut processed = 0u32;
    for candidate in candidates {
        processed = processed.saturating_add(1);
        on_progress(processed, total, &candidate.title);
        let path = candidate.path.clone();
        let chapters = tokio::task::spawn_blocking(move || extract_chapter_texts(&path))
            .await
            .unwrap_or_else(|join_err| {
                tracing::warn!(
                    book_uuid = %candidate.book_uuid,
                    %join_err,
                    is_panic = join_err.is_panic(),
                    "content extraction task failed; leaving unindexed"
                );
                None
            });
        let Some(chapters) = chapters else {
            tracing::warn!(
                book_uuid = %candidate.book_uuid,
                path = %candidate.path.display(),
                "content extraction could not read epub; will retry next scan"
            );
            continue;
        };
        replace_book_chapters(pool, &candidate, &chapters)
            .await
            .map_err(|e| anyhow::anyhow!("store content index for {}: {e}", candidate.book_uuid))?;
    }
    Ok(())
}

/// Delete-and-reinsert one book's chapter rows in a single transaction,
/// stamping each row with the snapshot the text was extracted from. The
/// migration's triggers mirror both halves into `book_content_fts`, so the
/// index and the content table move together or not at all.
async fn replace_book_chapters(
    pool: &SqlitePool,
    candidate: &Candidate,
    chapters: &[extract::ChapterText],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM book_content_chapters WHERE book_uuid = ?")
        .bind(&candidate.book_uuid)
        .execute(&mut *tx)
        .await?;
    for chapter in chapters {
        sqlx::query(
            "INSERT INTO book_content_chapters \
                 (book_uuid, spine_index, mtime_epoch, size_bytes, text) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&candidate.book_uuid)
        .bind(chapter.spine_index)
        .bind(candidate.mtime_epoch)
        .bind(candidate.size_bytes)
        .bind(&chapter.text)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests;
