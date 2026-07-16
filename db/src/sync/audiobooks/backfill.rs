//! Tail of the audiobook sync transaction: the stat-only `book_files`
//! backfill and the `scan_roots.last_indexed` stamp. Kept audiobook-local
//! (unlike the ebook path's `super::super::backfill::backfill_stat_chunks`)
//! since the two backfills bind different column sets.

use sqlx::Transaction;

use super::super::books::SyncError;

/// Apply the stat-only backfill: UPDATE `book_files.(mtime_epoch, size_bytes)`
/// in chunks of 250 (3 binds per row + library_id keeps us under SQLite's
/// 999-parameter cap). No OPF re-parse, no link writes, no FTS write.
pub(super) async fn backfill_audiobook_stats(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    backfill: &[(String, i64, i64)],
) -> Result<(), SyncError> {
    for chunk in backfill.chunks(250) {
        let rows = std::iter::repeat_n("(?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE book_files SET mtime_epoch = v.column2, size_bytes = v.column3 \
             FROM (VALUES {rows}) AS v, books b \
             WHERE b.uuid = v.column1 AND b.library_id = ? AND book_files.book_id = b.id"
        );
        let mut q = sqlx::query(&sql);
        for (uuid, mtime_epoch, size_bytes) in chunk {
            q = q.bind(uuid).bind(mtime_epoch).bind(size_bytes);
        }
        q = q.bind(library_id);
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Stamp `scan_roots.last_indexed` with the current unix epoch — the last
/// step inside the sync transaction.
pub(super) async fn stamp_audiobooks_last_indexed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
) -> Result<(), SyncError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query("UPDATE scan_roots SET last_indexed = ? WHERE id = ?")
        .bind(now)
        .bind(library_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
