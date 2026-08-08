//! Post-reindex backfill pipelines: fill `file_chapters` for audiobooks and
//! `books.word_count` for EPUBs indexed before those columns existed. Each
//! runs as a separate worker task after a library scan and is a no-op once
//! every book has been backfilled.

use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::{audiobook, covers, ebook, sync, thumbs};

/// Query the first-part filename (ordinal=0) and format for every book
/// under `library_path` that needs chapter backfill (no `file_chapters`
/// rows yet).
async fn fetch_backfill_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, String, String)>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT bf.id, bfp.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON bf.book_id = b.id \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_file_parts bfp ON bfp.book_file_id = bf.id \
         WHERE l.path = ? \
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
    mut on_progress: impl FnMut(u32, u32),
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

    let book_file_ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();
    let parts_by_id = bulk_fetch_parts(pool, &book_file_ids).await?;

    // Process books in batches of 250 to bound transaction size (mirrors the
    // sync/audiobooks.rs backfill pattern).
    let lib_root = PathBuf::from(library_path);
    for (batch_idx, chunk) in rows.chunks(250).enumerate() {
        let mut tx = pool.begin().await?;
        for (i, (book_file_id, first_part_filename, format)) in chunk.iter().enumerate() {
            let abs = lib_root.join(first_part_filename);
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
            on_progress(progress, total);
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
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let ids = fetch_word_count_candidates(pool, library_path).await?;
    if ids.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    tracing::info!(count = total, "backfilling word counts for existing ebooks");

    // One batched path lookup up front (chunked internally), same helper the
    // stats read path used to call per-request.
    let paths = crate::book_file_paths(pool, &ids, "EPUB").await?;

    let mut processed = 0u32;
    for chunk in ids.chunks(250) {
        let mut tx = pool.begin().await?;
        for &id in chunk {
            processed = processed.saturating_add(1);
            on_progress(processed, total);

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
            sqlx::query("UPDATE books SET word_count = ? WHERE id = ?")
                .bind(words)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
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
) -> anyhow::Result<Vec<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT b.id \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_files bf ON bf.book_id = b.id AND bf.format = 'EPUB' COLLATE NOCASE \
         WHERE l.path = ? AND b.word_count IS NULL \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(ids)
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
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let ids = fetch_page_count_candidates(pool, library_path).await?;
    if ids.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    tracing::info!(count = total, "backfilling page counts for existing comics");

    // One batched path lookup up front (chunked internally), same pattern
    // `backfill_word_counts` uses.
    let paths = crate::book_file_paths(pool, &ids, "CBZ").await?;

    let mut processed = 0u32;
    for chunk in ids.chunks(250) {
        let mut tx = pool.begin().await?;
        for &id in chunk {
            processed = processed.saturating_add(1);
            on_progress(processed, total);

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
            sqlx::query("UPDATE books SET page_count = ? WHERE id = ?")
                .bind(count)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
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
) -> anyhow::Result<Vec<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT b.id \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_files bf ON bf.book_id = b.id AND bf.format = 'CBZ' COLLATE NOCASE \
         WHERE l.path = ? AND b.page_count IS NULL \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// Pre-generate all three WebP thumbnail sizes for every book under
/// `library_path` that has a cover (#1752), so the landing grid's first
/// post-scan load serves cached thumbnails instead of falling through the
/// lazy generation path in `server::backend::covers::thumb_cache_miss_response`.
///
/// Posted as a separate worker task after each ebook library scan (mirroring
/// [`backfill_word_counts`]). Cheap when caught up: a book whose three sizes
/// are already fresh per [`thumbs::is_stale_async`] is skipped without touching its
/// cover bytes, so a re-scan of an unchanged library does no re-encoding —
/// exactly what [`thumbs::ensure_thumbnails_sync`] does for a single book.
///
/// Processes one book at a time (decode + encode is CPU-bound and runs on
/// the blocking pool via `spawn_blocking`) rather than fanning out, so a
/// full-library warm-up can't starve an interactive `/api/thumbs` request
/// for CPU. A per-book failure (missing cover, decode error, I/O) is logged
/// and skipped rather than aborting the batch — the next scan or an
/// interactive view retries it via the lazy path. `on_progress(processed,
/// total)` is called per book for the UI, mirroring the sibling backfills.
pub(crate) async fn backfill_thumbs(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let candidates = fetch_thumb_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    tracing::info!(
        count = total,
        "pre-generating thumbnails for existing covers"
    );

    let mut processed = 0u32;
    for (book_id, last_modified_epoch) in candidates {
        processed = processed.saturating_add(1);
        on_progress(processed, total);

        let mut all_fresh = true;
        for size in thumbs::ThumbSize::all() {
            if thumbs::is_stale_async(book_id, size, last_modified_epoch).await {
                all_fresh = false;
                break;
            }
        }
        if all_fresh {
            continue;
        }

        let cover = match covers::get_cover(pool, book_id).await {
            Ok(Some((_mime, bytes))) => bytes,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(
                    book_id,
                    error = %e,
                    "thumbnail backfill: cover fetch failed; skipping"
                );
                continue;
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

/// `(books.id, last_modified_epoch)` for every book under `library_path`
/// that has a cover — the [`backfill_thumbs`] work set. Scoped to the
/// scanned library so the follow-up task's cost tracks that scan.
/// Cover-override-only books (`has_cover = 0` with an uploaded override)
/// aren't included; they're left to the lazy `thumb_cache_miss_response`
/// path, same as before this backfill existed.
async fn fetch_thumb_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, i64)>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT b.id, CAST(COALESCE(b.last_modified, strftime('%s','now')) AS INTEGER) \
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
