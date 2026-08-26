//! Post-reindex backfill pipelines: fill `file_chapters` for audiobooks and
//! `books.word_count` for EPUBs indexed before those columns existed. Each
//! runs as a separate worker task after a library scan and is a no-op once
//! every book has been backfilled.

use std::path::PathBuf;

use sqlx::{SqlitePool, Transaction};

use crate::{audiobook, covers, ebook, sync, thumbs};

/// Rows per chunk for [`batch_update_books_column`]. Each row costs three
/// binds (two in the `CASE`, one in the `IN` list), so 200 keeps a chunk
/// comfortably under SQLite's 999-parameter cap — mirrors `ORDINAL_CHUNK` in
/// `db/src/merge/transaction.rs`.
const BACKFILL_CHUNK: usize = 200;

/// The `books` columns [`batch_update_books_column`] can write. A closed
/// enum rather than a bare column-name `&str`, so the SQL-interpolation
/// site can never be handed an arbitrary or user-controlled string.
enum BooksColumn {
    WordCount,
    PageCount,
}

impl BooksColumn {
    fn as_sql(&self) -> &'static str {
        match self {
            BooksColumn::WordCount => "word_count",
            BooksColumn::PageCount => "page_count",
        }
    }
}

/// Write `updates` (`book id -> new value`) into `books.<column>` via one
/// `CASE`-based UPDATE per chunk, replacing what would otherwise be one
/// `UPDATE ... WHERE id = ?` per row. A no-op on an empty `updates` slice,
/// so callers don't need their own guard.
async fn batch_update_books_column(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    column: BooksColumn,
    updates: &[(i64, i64)],
) -> Result<(), sqlx::Error> {
    let column = column.as_sql();
    for chunk in updates.chunks(BACKFILL_CHUNK) {
        let cases = std::iter::repeat_n("WHEN ? THEN ?", chunk.len())
            .collect::<Vec<_>>()
            .join(" ");
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("UPDATE books SET {column} = CASE id {cases} END WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for (id, value) in chunk {
            q = q.bind(id).bind(value);
        }
        for (id, _) in chunk {
            q = q.bind(id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Query the first-part filename (ordinal=0), format, and effective scan
/// root for every book under `library_path` that needs chapter backfill (no
/// `file_chapters` rows yet).
///
/// Scoped on the file's own root (`COALESCE(bf.library_path, l.path)`, the
/// shape `book_file_path` resolves with), not the book's: a merged
/// audiobook's files hang off a target book whose `library_id` is the
/// *target's* scan root, so scoping on the book's would drop every merged
/// audiobook from the backfill.
async fn fetch_backfill_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, String, String, String)>> {
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT bf.id, bfp.filename, bf.format, COALESCE(bf.library_path, l.path) \
         FROM book_files bf \
         JOIN books b ON bf.book_id = b.id \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_file_parts bfp ON bfp.book_file_id = bf.id \
         WHERE COALESCE(bf.library_path, l.path) = ? \
           AND bf.format IN ('M4B', 'M4A', 'MP3') \
           AND bfp.ordinal = 0 \
           AND NOT EXISTS (SELECT 1 FROM file_chapters fc WHERE fc.book_file_id = bf.id) \
         ORDER BY bf.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Bulk-fetch all `book_file_parts` rows for `book_file_ids` and group them
/// by `book_file_id` — avoids N per-book SELECT round-trips. Chunked at 500
/// to stay well under SQLite's 999 bind-parameter limit.
async fn bulk_fetch_parts(
    pool: &SqlitePool,
    book_file_ids: &[i64],
) -> anyhow::Result<std::collections::HashMap<i64, Vec<audiobook::AudiobookPart>>> {
    let mut all_parts_rows: Vec<(i64, i64, String, i64, i64, f64)> = Vec::new();
    for chunk in book_file_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let parts_sql = format!(
            "SELECT book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds \
             FROM book_file_parts WHERE book_file_id IN ({placeholders}) \
             ORDER BY book_file_id, ordinal"
        );
        let mut parts_query = sqlx::query_as::<_, (i64, i64, String, i64, i64, f64)>(&parts_sql);
        for id in chunk {
            parts_query = parts_query.bind(id);
        }
        all_parts_rows.extend(parts_query.fetch_all(pool).await?);
    }

    let mut parts_by_id: std::collections::HashMap<i64, Vec<audiobook::AudiobookPart>> =
        std::collections::HashMap::new();
    for (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) in
        all_parts_rows
    {
        parts_by_id
            .entry(book_file_id)
            .or_default()
            .push(audiobook::AudiobookPart {
                ordinal,
                filename,
                size_bytes,
                mtime_epoch,
                duration_seconds,
            });
    }
    Ok(parts_by_id)
}

/// Fill `file_chapters` for audiobook `book_files` rows that have none.
///
/// The chapter extraction pipeline was added after the initial audiobook
/// indexer, so books indexed before the migration have zero `file_chapters`
/// rows. The normal diff-based reindex skips unchanged files, so this
/// backfill runs as a separate worker task and is a no-op once all books
/// have chapters. `on_progress(processed, total)` is called after each
/// book so the UI can render a progress bar.
///
/// ## Query efficiency
///
/// All `book_file_parts` rows for the backfill set are fetched in a single
/// `WHERE book_file_id IN (…)` bulk query before the loop rather than one
/// per book, and all chapter inserts are committed in batches of 250 books
/// to avoid per-book WAL flushes (mirrors the sync/audiobooks.rs backfill
/// pattern).
pub(crate) async fn backfill_chapters(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    let rows = fetch_backfill_candidates(pool, library_path).await?;
    if rows.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    tracing::info!(
        count = total,
        "backfilling chapters for existing audiobooks"
    );

    let book_file_ids: Vec<i64> = rows.iter().map(|(id, ..)| *id).collect();
    let parts_by_id = bulk_fetch_parts(pool, &book_file_ids).await?;

    // Process books in batches of 250 to bound transaction size (mirrors the
    // sync/audiobooks.rs backfill pattern).
    let root_name = super::root_display_name(library_path);
    for (batch_idx, chunk) in rows.chunks(250).enumerate() {
        let mut tx = pool.begin().await?;
        for (i, (book_file_id, first_part_filename, format, file_root)) in chunk.iter().enumerate()
        {
            // The row's own root, not the task's: a stat under the wrong
            // root extracts nothing and writes the synthetic `Part N`
            // fallback over the book's real chapters.
            let abs = PathBuf::from(file_root).join(first_part_filename);
            let fmt = format.clone();
            let chapters =
                tokio::task::spawn_blocking(move || audiobook::extract_chapters(&abs, &fmt))
                    .await
                    .unwrap_or_else(|join_err| {
                        tracing::warn!(
                            book_file_id,
                            %join_err,
                            is_panic = join_err.is_panic(),
                            "chapter extraction task failed; using synthetic fallback"
                        );
                        Vec::new()
                    });

            let parts = parts_by_id
                .get(book_file_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            sync::insert_chapters(&mut tx, *book_file_id, &chapters, parts).await?;

            let global_idx = batch_idx * 250 + i;
            let progress = u32::try_from(global_idx)
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            on_progress(
                progress,
                total,
                &super::display_item(&root_name, first_part_filename),
            );
        }
        tx.commit().await?;
    }

    Ok(())
}

/// Fill `books.word_count` for EPUB-backed books under `library_path` that
/// have none yet (NULL `word_count`).
///
/// Word counts were added after the initial ebook indexer, so books indexed
/// before migration `0049` carry a NULL. The normal diff-based reindex only
/// re-parses new/changed files, so it never revisits them; this backfill runs
/// as a separate worker task posted after each library scan (mirroring
/// [`backfill_chapters`]) and is a no-op once every EPUB book has a count.
/// `on_progress(processed, total)` is called per book for the UI.
///
/// Each candidate's EPUB is opened and word-counted on the blocking pool
/// (zip/XML work), and updates commit in batches of 250 to bound WAL flushes.
/// A book whose spine can't be estimated stays NULL and is retried on the next
/// scan — rare, and cheaper than persisting a sentinel that lies about the
/// estimate.
pub(crate) async fn backfill_word_counts(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    let candidates = fetch_word_count_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    tracing::info!(count = total, "backfilling word counts for existing ebooks");

    // One batched path lookup up front (chunked internally), same helper the
    // stats read path used to call per-request.
    let ids: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
    let paths = crate::book_file_paths(pool, &ids, "EPUB").await?;

    let mut processed = 0u32;
    for chunk in candidates.chunks(250) {
        let mut tx = pool.begin().await?;
        let mut updates = Vec::with_capacity(chunk.len());
        for (id, title) in chunk {
            let id = *id;
            processed = processed.saturating_add(1);
            on_progress(processed, total, title);

            let Some(path) = paths.get(&id).cloned() else {
                continue;
            };
            let words = tokio::task::spawn_blocking(move || {
                epub::doc::EpubDoc::new(&path)
                    .ok()
                    .and_then(|mut doc| ebook::estimate_word_count(&mut doc))
            })
            .await
            .unwrap_or_else(|join_err| {
                tracing::warn!(
                    book_id = id,
                    %join_err,
                    is_panic = join_err.is_panic(),
                    "word-count task failed; leaving word_count NULL"
                );
                None
            });

            let Some(words) = words else { continue };
            updates.push((id, words));
        }
        // One CASE-based UPDATE for the whole 250-row chunk instead of one
        // UPDATE per book, keeping the existing tx boundary that bounds WAL
        // flushes.
        batch_update_books_column(&mut tx, BooksColumn::WordCount, &updates).await?;
        tx.commit().await?;
    }

    Ok(())
}

/// `books.id` for every EPUB-backed book under `library_path` still missing a
/// `word_count` — the [`backfill_word_counts`] work set. Scoped to the scanned
/// library so the follow-up task's cost tracks that scan.
async fn fetch_word_count_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, String)>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT b.id, COALESCE(NULLIF(b.title, ''), b.scan_key) \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_files bf ON bf.book_id = b.id AND bf.format = 'EPUB' COLLATE NOCASE \
         WHERE l.path = ? AND b.word_count IS NULL \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fill `books.page_count` for CBZ-backed books under `library_path` that
/// have none yet (NULL `page_count`).
///
/// Page counts were added after the initial comic indexer, so books indexed
/// before that carry a NULL. The normal diff-based reindex only re-parses
/// new/changed files, so it never revisits them; this backfill runs as a
/// separate worker task posted after each ebook library scan (mirroring
/// [`backfill_word_counts`]) and is a
/// no-op once every CBZ book has a count. `on_progress(processed, total)` is
/// called per book for the UI.
///
/// Each candidate's archive is listed (central-directory read, not a
/// decompression) on the blocking pool, and updates commit in batches of
/// 250 to bound WAL flushes. A book whose archive can't be listed stays
/// NULL and is retried on the next scan.
pub(crate) async fn backfill_page_counts(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    let candidates = fetch_page_count_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    tracing::info!(count = total, "backfilling page counts for existing comics");

    // One batched path lookup up front (chunked internally), same pattern
    // `backfill_word_counts` uses.
    let ids: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
    let paths = crate::book_file_paths(pool, &ids, "CBZ").await?;

    let mut processed = 0u32;
    for chunk in candidates.chunks(250) {
        let mut tx = pool.begin().await?;
        let mut updates = Vec::with_capacity(chunk.len());
        for (id, title) in chunk {
            let id = *id;
            processed = processed.saturating_add(1);
            on_progress(processed, total, title);

            let Some(path) = paths.get(&id).cloned() else {
                continue;
            };
            let count = tokio::task::spawn_blocking(move || {
                crate::comic::list_pages(&path)
                    .ok()
                    .map(|pages| pages.len() as i64)
            })
            .await
            .unwrap_or_else(|join_err| {
                tracing::warn!(
                    book_id = id,
                    %join_err,
                    is_panic = join_err.is_panic(),
                    "page-count task failed; leaving page_count NULL"
                );
                None
            });

            let Some(count) = count else { continue };
            updates.push((id, count));
        }
        // One CASE-based UPDATE for the whole 250-row chunk instead of one
        // UPDATE per book, keeping the existing tx boundary that bounds WAL
        // flushes.
        batch_update_books_column(&mut tx, BooksColumn::PageCount, &updates).await?;
        tx.commit().await?;
    }

    Ok(())
}

/// `books.id` for every CBZ-backed book under `library_path` still missing a
/// `page_count` — the [`backfill_page_counts`] work set. Scoped to the
/// scanned library so the follow-up task's cost tracks that scan.
async fn fetch_page_count_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, String)>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT b.id, COALESCE(NULLIF(b.title, ''), b.scan_key) \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_files bf ON bf.book_id = b.id AND bf.format = 'CBZ' COLLATE NOCASE \
         WHERE l.path = ? AND b.page_count IS NULL \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Pre-generate all three WebP thumbnail sizes for every book under
/// `library_path` that has a cover (#1752), so the landing grid's first
/// post-scan load serves cached thumbnails instead of falling through the
/// lazy generation path in `server::backend::covers::thumb_cache_miss_response`.
///
/// Posted as a separate worker task after each ebook library scan (mirroring
/// [`backfill_word_counts`]). Cheap when caught up: candidates are first
/// partitioned by [`thumbs::is_stale_async`] into the subset actually needing
/// a re-encode (#1817) — a book with all three sizes already fresh never
/// touches its cover bytes, is never logged, and never advances the reported
/// `total`, so a re-scan of an unchanged library posts no visible progress
/// task at all.
///
/// Processes one book at a time (decode + encode is CPU-bound and runs on
/// the blocking pool via `spawn_blocking`) rather than fanning out, so a
/// full-library warm-up can't starve an interactive `/api/thumbs` request
/// for CPU. A per-book failure (missing cover, decode error, I/O) is logged
/// and skipped rather than aborting the batch — the next scan or an
/// interactive view retries it via the lazy path. `on_progress(processed,
/// total)` is called once per stale book for the UI, mirroring the sibling
/// backfills.
pub(crate) async fn backfill_thumbs(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    let candidates = fetch_thumb_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }

    let mut stale = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if is_any_thumb_stale(candidate.0, candidate.1).await {
            stale.push(candidate);
        }
    }
    if stale.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(stale.len()).unwrap_or(u32::MAX);
    tracing::info!(
        count = total,
        "pre-generating thumbnails for existing covers"
    );

    let mut processed = 0u32;
    for (book_id, last_modified_epoch, title) in stale {
        processed = processed.saturating_add(1);
        on_progress(processed, total, &title);
        regenerate_thumbs(pool, book_id, last_modified_epoch).await;
    }

    let cap = thumbs::cap_bytes();
    match tokio::task::spawn_blocking(move || thumbs::evict_if_over_cap(cap)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "thumbnail backfill: eviction failed"),
        Err(join_err) => tracing::warn!(
            %join_err,
            is_panic = join_err.is_panic(),
            "thumbnail backfill: eviction task failed"
        ),
    }

    Ok(())
}

/// Whether any of a book's three thumbnail sizes is stale per
/// [`thumbs::is_stale_async`] — the [`backfill_thumbs`] partition predicate.
async fn is_any_thumb_stale(book_id: i64, last_modified_epoch: i64) -> bool {
    for size in thumbs::ThumbSize::all() {
        if thumbs::is_stale_async(book_id, size, last_modified_epoch).await {
            return true;
        }
    }
    false
}

/// Regenerate one book's three thumbnail sizes, skipping (and logging) a
/// missing cover, fetch failure, or generation failure rather than aborting
/// the batch. Callers are expected to have already established via
/// [`is_any_thumb_stale`] that this book needs re-encoding.
async fn regenerate_thumbs(pool: &SqlitePool, book_id: i64, last_modified_epoch: i64) {
    let cover = match covers::get_cover(pool, book_id).await {
        Ok(Some((_mime, bytes))) => bytes,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                book_id,
                error = %e,
                "thumbnail backfill: cover fetch failed; skipping"
            );
            return;
        }
    };

    match tokio::task::spawn_blocking(move || {
        thumbs::ensure_thumbnails_sync(book_id, last_modified_epoch, cover)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(
                book_id,
                error = %e,
                "thumbnail backfill: generation failed; skipping"
            );
        }
        Err(join_err) => {
            tracing::warn!(
                book_id,
                %join_err,
                is_panic = join_err.is_panic(),
                "thumbnail backfill: generation task failed; skipping"
            );
        }
    }
}

/// `(books.id, last_modified_epoch)` for every book under `library_path`
/// that has a cover — the [`backfill_thumbs`] work set. Scoped to the
/// scanned library so the follow-up task's cost tracks that scan.
/// Cover-override-only books (`has_cover = 0` with an uploaded override)
/// aren't included; they're left to the lazy `thumb_cache_miss_response`
/// path, same as before this backfill existed.
async fn fetch_thumb_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, i64, String)>> {
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT b.id, CAST(COALESCE(b.last_modified, strftime('%s','now')) AS INTEGER), \
                COALESCE(NULLIF(b.title, ''), b.scan_key) \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         WHERE l.path = ? AND b.has_cover = 1 \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Every EPUB `book_files` row under `library_path` with no
/// `epub_spine_stats` yet, carrying the resolved on-disk path — the same
/// path shape `book_file_path` builds, inlined so one query serves the
/// whole candidate set with per-file (not per-book lowest-ordinal) paths.
async fn fetch_epub_structure_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, String, PathBuf)>> {
    let rows: Vec<(i64, String, String, String, String, String)> = sqlx::query_as(
        "SELECT bf.id, COALESCE(NULLIF(b.title, ''), b.scan_key), \
                COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON bf.book_id = b.id \
         JOIN scan_roots l ON b.library_id = l.id \
         WHERE l.path = ? \
           AND bf.format = 'EPUB' COLLATE NOCASE \
           AND NOT EXISTS (SELECT 1 FROM epub_spine_stats s WHERE s.book_file_id = bf.id) \
         ORDER BY bf.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, lib, dir, stem, fmt)| {
            let path = std::path::Path::new(&lib)
                .join(&dir)
                .join(format!("{stem}.{}", fmt.to_lowercase()));
            (id, title, path)
        })
        .collect())
}

/// Fill `epub_spine_stats` + `ebook_chapters` for every EPUB file that has
/// none. The NOT EXISTS predicate keys on the stats table, which extraction
/// always writes for a readable book — so an honestly TOC-less book stores
/// stats plus zero chapters and is done, while an unreadable file stores
/// nothing and is retried on the next scan (the `backfill_word_counts`
/// NULL semantics).
pub(crate) async fn backfill_epub_structure(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    let candidates = fetch_epub_structure_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }
    let total = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    tracing::info!(
        count = total,
        "backfilling epub structure for existing ebooks"
    );

    let mut processed = 0u32;
    for (book_file_id, title, path) in candidates {
        processed = processed.saturating_add(1);
        on_progress(processed, total, &title);
        let structure = tokio::task::spawn_blocking(move || {
            epub::doc::EpubDoc::new(&path)
                .ok()
                .and_then(|mut doc| ebook::toc::extract_structure(&mut doc))
        })
        .await
        .unwrap_or_else(|join_err| {
            tracing::warn!(
                book_file_id,
                %join_err,
                is_panic = join_err.is_panic(),
                "epub structure task failed; leaving unextracted"
            );
            None
        });
        let Some(structure) = structure else { continue };
        crate::epub_structure::replace_structure(pool, book_file_id, &structure)
            .await
            .map_err(|e| anyhow::anyhow!("store epub structure for file {book_file_id}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::init_db;
    use crate::test_support::seed_minimal_books;

    /// Empty `updates` must not build any SQL at all — `chunks()` over an
    /// empty slice yields zero chunks, so this is really asserting the
    /// no-op holds rather than that some degenerate `CASE`/`IN ()` executes.
    #[tokio::test]
    async fn batch_update_books_column_is_a_noop_for_empty_updates() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        seed_minimal_books(&pool, 1).await;

        let mut tx = pool.begin().await.unwrap();
        batch_update_books_column(&mut tx, BooksColumn::WordCount, &[])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let word_count: Option<i64> =
            sqlx::query_scalar("SELECT word_count FROM books WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(word_count, None);
    }

    /// A single-row update must produce valid `CASE id WHEN ? THEN ? END`
    /// and `IN (?)` clauses, not just the multi-row shape.
    #[tokio::test]
    async fn batch_update_books_column_writes_a_single_row() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        seed_minimal_books(&pool, 1).await;

        let mut tx = pool.begin().await.unwrap();
        batch_update_books_column(&mut tx, BooksColumn::WordCount, &[(1, 42)])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let word_count: Option<i64> =
            sqlx::query_scalar("SELECT word_count FROM books WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(word_count, Some(42));
    }

    /// A multi-row update must resolve each id to its own value via the
    /// `CASE` branches, not clobber every matched row with one value.
    #[tokio::test]
    async fn batch_update_books_column_writes_distinct_values_per_row() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        seed_minimal_books(&pool, 3).await;

        let mut tx = pool.begin().await.unwrap();
        batch_update_books_column(
            &mut tx,
            BooksColumn::PageCount,
            &[(1, 10), (2, 20), (3, 30)],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        for (id, expected) in [(1, 10), (2, 20), (3, 30)] {
            let page_count: Option<i64> =
                sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(page_count, Some(expected));
        }
    }
}
