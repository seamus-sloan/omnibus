//! Multi-file audiobook sync. Mirrors `sync_books` but also writes
//! `book_file_parts`; all `books_fts` maintenance routes through
//! [`super::fts`]. Split per bucket like [`super::books`]: [`changed`] /
//! [`new`] / [`removed`] apply their bucket, [`shared`] holds the row
//! writers + attach path, [`backfill`] the stat-only backfill + stamp.

use sqlx::SqlitePool;

use crate::covers::delete_cover_files_for;
use crate::settings::upsert_library;

use super::books::{materialize_new_covers, sync_moved, MovedFile};

mod backfill;
mod changed;
mod new;
mod removed;
mod shared;

#[cfg(test)]
mod tests;

pub(crate) use shared::insert_chapters;

use backfill::{backfill_audiobook_stats, stamp_audiobooks_last_indexed};
use changed::sync_audiobooks_changed;
use new::sync_audiobooks_new;
use removed::sync_audiobooks_removed;

/// Per-bucket payload for [`sync_audiobooks`]. Mirrors [`SyncPlan`] for
/// the ebook path but carries [`crate::audiobook::IndexedAudiobook`] rows
/// that include the ordered `book_file_parts` list.
#[derive(Debug, Default)]
pub struct AudiobookSyncPlan {
    pub new_books: Vec<crate::audiobook::IndexedAudiobook>,
    pub changed_books: Vec<crate::audiobook::IndexedAudiobook>,
    /// Groups that changed directory but not content. Applied by the same
    /// writer the ebook pipeline uses — a relocated group differs only in
    /// having `book_file_parts` rows to carry along.
    pub moved: Vec<MovedFile>,
    pub removed_uuids: Vec<String>,
    /// `(uuid, mtime_epoch, size_bytes)` stat-only backfill (no re-parse).
    pub backfill: Vec<(String, i64, i64)>,
}

/// Apply a multi-file audiobook sync plan atomically. Mirrors [`sync_books`]
/// but writes `book_file_parts` rows in addition to `books` and `book_files`.
///
/// Transaction order:
/// 1. Upsert `scan_roots` row.
/// 2. Mark Removed files missing (drop `book_files`, retain the `books` row).
/// 3. Relocate Moved groups: repoint path columns (including every
///    `book_file_parts.filename`) without re-parsing.
/// 4. Update Changed in-place: wipe `book_files` + `book_file_parts` + author
///    link + FTS, then re-insert them.
/// 5. Insert New.
/// 6. Backfill `book_files.(mtime_epoch, size_bytes)` only.
/// 7. Delete taxonomy rows left with zero books by step 4's link wipe.
/// 8. Stamp `scan_roots.last_indexed`.
///
/// Post-commit: write / delete cover files (best-effort, same as sync_books).
///
/// [`sync_books`]: super::books::sync_books
pub async fn sync_audiobooks(
    pool: &SqlitePool,
    library_path: &str,
    plan: AudiobookSyncPlan,
) -> anyhow::Result<()> {
    sync_audiobooks_with_progress(pool, library_path, plan, |_, _| {}).await
}

/// [`sync_audiobooks`] variant that calls `on_progress(processed, total)`
/// after each per-book write. `total` counts the buckets that loop per
/// book — Changed + New. Removed and Backfill are batched and not
/// reported as per-book ticks.
pub async fn sync_audiobooks_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    plan: AudiobookSyncPlan,
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let total: u32 = (plan.changed_books.len() + plan.new_books.len())
        .try_into()
        .unwrap_or(u32::MAX);
    // Emit (0, total) before any per-book work so the UI flips from
    // indeterminate spinner to determinate bar on the first poll.
    on_progress(0, total);
    let mut processed: u32 = 0;

    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    sync_audiobooks_removed(&mut tx, library_id, &plan.removed_uuids).await?;
    // Before New, for the reason `sync_books` documents: a relocation frees
    // no attach slot, but a new file inserted first could occupy the one the
    // moved group is about to want.
    sync_moved(&mut tx, library_id, library_path, &plan.moved).await?;
    let changed_covers = sync_audiobooks_changed(
        &mut tx,
        library_id,
        library_path,
        &plan.changed_books,
        || {
            processed = processed.saturating_add(1);
            on_progress(processed, total);
        },
    )
    .await?;
    let new_covers = sync_audiobooks_new(
        &mut tx,
        library_id,
        library_path,
        &plan.new_books,
        &plan.removed_uuids,
        || {
            processed = processed.saturating_add(1);
            on_progress(processed, total);
        },
    )
    .await?;
    backfill_audiobook_stats(&mut tx, library_id, &plan.backfill).await?;
    // Only Changed wipes link rows before re-inserting, so it's the only bucket
    // that can leave an author or series with zero books. See `sync_books`.
    if !plan.changed_books.is_empty() {
        crate::taxonomy::delete_orphan_taxonomy(&mut tx).await?;
    }
    stamp_audiobooks_last_indexed(&mut tx, library_id).await?;

    tx.commit().await?;

    let removed_uuids = plan.removed_uuids;
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        delete_cover_files_for(&removed_uuids);
        materialize_new_covers(new_covers);
        materialize_new_covers(changed_covers);
    })
    .await
    {
        tracing::error!("sync_audiobooks: cover reconcile spawn_blocking failed: {join_err}");
    }

    Ok(())
}
