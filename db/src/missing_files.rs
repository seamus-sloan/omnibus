//! Garbage-collects `books` rows whose files have disappeared. A removed file
//! leaves its row in place (fileless, so a returning file re-links by its
//! relative path); without a GC those rows accumulate. [`gc_books_missing_files`]
//! (run by the indexer after each reindex) purges long-missing, user-data-free
//! rows; [`backfill_missing_files_flags`] stamps pre-existing fileless rows at boot.

use sqlx::SqlitePool;

use crate::covers::delete_cover_files_for;
use crate::metadata_overrides::delete_override_cover;

#[cfg(test)]
mod tests;

/// Days a book may be missing its files before it becomes eligible for GC.
pub const MISSING_FILES_RETENTION_DAYS: i64 = 30;

/// Errors returned by the missing-files GC entry points.
#[derive(Debug, thiserror::Error)]
pub enum MissingFilesError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Hard-delete books whose files have been missing longer than
/// `retention_days`, **except** any that carry user data (a row in one of the
/// soft-ref user-data tables), are intentionally fileless
/// (`is_missing_files_override = 1`), or still anchor a cross-format
/// attachment (`merged_uuids`). Each purged book's regenerable
/// `metadata_overrides` row and cover files go with it. Returns the purged
/// count.
///
/// Best-effort cover unlink runs off the runtime via `spawn_blocking`; a
/// `JoinError` there is logged and swallowed — covers are a rebuildable cache,
/// so a failed unlink must not fail the GC (mirrors `set_settings`).
pub async fn gc_books_missing_files(
    pool: &SqlitePool,
    retention_days: i64,
) -> Result<u64, MissingFilesError> {
    // `has_file = 0` (the NOT EXISTS book_files arm) is the authoritative
    // fileless test, so a stale `is_missing_files` flag on a re-attached book can
    // never cause a wrong purge.
    let victims: Vec<(i64, String)> = sqlx::query_as(
        "SELECT b.id, b.uuid FROM books b
          WHERE b.is_missing_files = 1
            AND b.is_missing_files_override = 0
            AND b.missing_files_since < unixepoch('now', '-' || ? || ' days')
            AND NOT EXISTS (SELECT 1 FROM book_files       bf WHERE bf.book_id = b.id)
            AND NOT EXISTS (SELECT 1 FROM physical_copies   pc WHERE pc.book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM merged_uuids      m  WHERE m.book_id  = b.id)
            AND NOT EXISTS (SELECT 1 FROM reading_progress     WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM bookmarks            WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM reading_sessions     WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM listening_sessions   WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM annotations          WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM user_ratings         WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM book_read_status     WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM audiobook_playback_preferences
                                                        WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM journal_entries      WHERE book_uuid = b.uuid)",
    )
    .bind(retention_days)
    .fetch_all(pool)
    .await?;

    if victims.is_empty() {
        return Ok(0);
    }

    delete_victim_rows(pool, &victims).await?;

    let purged = victims.len() as u64;
    let uuids: Vec<String> = victims.into_iter().map(|(_, uuid)| uuid).collect();
    cleanup_victim_covers(uuids).await;

    Ok(purged)
}

/// Hard-delete the `(id, uuid)` victims in one transaction: their
/// `metadata_overrides`, `books`, and `books_fts` rows (chunked at 500 to
/// stay under SQLite's 999-bind cap), then any taxonomy the purge orphaned.
async fn delete_victim_rows(
    pool: &SqlitePool,
    victims: &[(i64, String)],
) -> Result<(), MissingFilesError> {
    let mut tx = pool.begin().await?;
    // `metadata_overrides` keys on uuid, `books` on id, so bind both per chunk.
    for chunk in victims.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        let del_overrides =
            format!("DELETE FROM metadata_overrides WHERE book_uuid IN ({placeholders})");
        let mut q = sqlx::query(&del_overrides);
        for (_, uuid) in chunk {
            q = q.bind(uuid);
        }
        q.execute(&mut *tx).await?;

        let del_books = format!("DELETE FROM books WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&del_books);
        for (id, _) in chunk {
            q = q.bind(id);
        }
        q.execute(&mut *tx).await?;

        // `books_fts` has no FK to `books`; a missing book stays searchable, so
        // its search twin (rowid = book id) only leaves the index when the book
        // is hard-deleted here.
        let del_fts = format!("DELETE FROM books_fts WHERE rowid IN ({placeholders})");
        let mut q = sqlx::query(&del_fts);
        for (id, _) in chunk {
            q = q.bind(id);
        }
        q.execute(&mut *tx).await?;
    }
    // Authors / series / tags / publishers / languages the purge left with zero
    // books are removed too (a book a user kept reading is never purged, so its
    // taxonomy survives with it).
    crate::taxonomy::delete_orphan_taxonomy(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Best-effort unlink of each purged book's override + scanned cover files,
/// off the runtime via `spawn_blocking`. A `JoinError` is logged and swallowed
/// — covers are a rebuildable cache, so a failed unlink must not fail the GC
/// (mirrors `set_settings`).
async fn cleanup_victim_covers(uuids: Vec<String>) {
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        for uuid in &uuids {
            delete_override_cover(uuid);
        }
        delete_cover_files_for(&uuids);
    })
    .await
    {
        tracing::error!("gc_books_missing_files: cover cleanup spawn_blocking failed: {join_err}");
    }
}

/// One-time stamp of `is_missing_files` / `missing_files_since` for rows that
/// were already fileless before migration 0029, starting their retention clock
/// at boot time. Idempotent (the `is_missing_files = 0` guard skips
/// already-stamped rows) and a no-op once caught up, so it runs every boot from
/// `init_db`.
pub async fn backfill_missing_files_flags(pool: &SqlitePool) -> Result<(), MissingFilesError> {
    sqlx::query(
        "UPDATE books
            SET is_missing_files = 1, missing_files_since = unixepoch()
          WHERE is_missing_files = 0
            AND is_missing_files_override = 0
            AND NOT EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)",
    )
    .execute(pool)
    .await?;
    Ok(())
}
