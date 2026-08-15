//! Data layer for the admin server-health page: index status, worker queue
//! depth, FTS index health, and storage utilization. Composed into one
//! [`AdminHealthReport`] by [`build_report`], which also folds in the
//! existing error-ring snapshot (the fifth section) so a single call backs
//! the whole page.

use omnibus_shared::admin_health::{
    AdminHealthReport, FtsHealth, IndexStatus, StorageStats, WorkerQueueEntry,
};
use sqlx::SqlitePool;

use crate::worker::Worker;

#[cfg(test)]
mod tests;

/// Failure space of the admin-health data layer. Every query here is a
/// plain read against the existing schema, so a wrapped `sqlx::Error` is
/// the only failure mode.
#[derive(Debug, thiserror::Error)]
pub enum AdminHealthError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Settings(#[from] crate::settings::SettingsError),
}

/// Library index summary: configured paths, row counts, and how many
/// `books` rows have been ghosted (no surviving `book_files`).
pub async fn index_status(pool: &SqlitePool) -> Result<IndexStatus, AdminHealthError> {
    let settings = crate::get_settings(pool).await?;
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(pool)
        .await?;
    let author_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(pool)
        .await?;
    let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(pool)
        .await?;
    let ghosted_book_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM books b \
         WHERE NOT EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)",
    )
    .fetch_one(pool)
    .await?;
    Ok(IndexStatus {
        ebook_library_path: settings.ebook_library_path,
        audiobook_library_path: settings.audiobook_library_path,
        book_count,
        author_count,
        series_count,
        ghosted_book_count,
    })
}

/// `books_fts` vs `books` row-count parity.
pub async fn fts_health(pool: &SqlitePool) -> Result<FtsHealth, AdminHealthError> {
    let books_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(pool)
        .await?;
    let fts_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(pool)
        .await?;
    Ok(FtsHealth {
        books_rows,
        fts_rows,
        in_sync: books_rows == fts_rows,
    })
}

/// Disk usage across the indexed library and every on-disk cache. The
/// filesystem walks run inside `spawn_blocking` since they're synchronous
/// I/O; a walk that fails to join (extremely unlikely — only a panicking
/// blocking task causes this) reports zero for that cache rather than
/// failing the whole report.
pub async fn storage_stats(pool: &SqlitePool) -> Result<StorageStats, AdminHealthError> {
    let library_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM book_files")
            .fetch_one(pool)
            .await?;
    let (covers_bytes, thumbs_bytes, hls_bytes, export_epub_bytes) =
        tokio::task::spawn_blocking(|| {
            (
                dir_size_flat(&crate::covers::covers_dir()),
                crate::thumbs::used_bytes(),
                crate::hls::used_bytes(),
                crate::epub_rewrite::used_bytes(),
            )
        })
        .await
        .unwrap_or((0, 0, 0, 0));
    Ok(StorageStats {
        library_bytes,
        covers_bytes,
        thumbs_bytes,
        thumbs_cap_bytes: crate::thumbs::cap_bytes(),
        hls_bytes,
        hls_cap_bytes: crate::hls::cap_bytes(),
        export_epub_bytes,
        export_epub_cap_bytes: crate::epub_rewrite::cap_bytes(),
    })
}

/// Sum the sizes of every regular file directly under `dir` (no
/// recursion — `covers_dir()` is a flat `<uuid>.<ext>` layout). A missing
/// directory or an unreadable entry reads as empty/skipped, matching
/// `thumbs::used_bytes`'s tolerance — this is a display figure, not an
/// eviction decision.
fn dir_size_flat(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

/// Live worker queue depth and recent completion timings, one entry per
/// [`omnibus_shared::TaskKind`] with at least one queued/running task or a
/// recorded completion. Pure projection of [`Worker::metrics`] — no I/O, so
/// no error path.
pub fn worker_queue_status(worker: &Worker) -> Vec<WorkerQueueEntry> {
    let metrics = worker.metrics();
    // `TaskKind` carries no `Ord` (it's a wire-shape enum, not a sort key —
    // see `shared/src/worker.rs`), so de-dupe via a `HashSet` and sort the
    // materialized entries by their `Debug` label instead, for a
    // deterministic, test-friendly order.
    let mut kinds: std::collections::HashSet<omnibus_shared::TaskKind> =
        metrics.queue_depth.keys().copied().collect();
    kinds.extend(metrics.recent_completions.keys().copied());
    let mut entries: Vec<WorkerQueueEntry> = kinds
        .into_iter()
        .map(|kind| WorkerQueueEntry {
            kind,
            queue_depth: metrics.queue_depth.get(&kind).copied().unwrap_or(0),
            recent_completions_ms: metrics
                .recent_completions
                .get(&kind)
                .map(|timings| {
                    timings
                        .iter()
                        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    entries.sort_by_key(|e| format!("{:?}", e.kind));
    entries
}

/// Assemble the full report backing `/admin/health`'s five sections in one
/// call: index status, worker queue, FTS health, storage, and the existing
/// in-memory error ring (`crate::error_ring::snapshot`).
pub async fn build_report(
    pool: &SqlitePool,
    worker: &Worker,
) -> Result<AdminHealthReport, AdminHealthError> {
    let index = index_status(pool).await?;
    let fts = fts_health(pool).await?;
    let storage = storage_stats(pool).await?;
    Ok(AdminHealthReport {
        index,
        worker_queue: worker_queue_status(worker),
        fts,
        storage,
        last_errors: crate::error_ring::snapshot(),
    })
}
