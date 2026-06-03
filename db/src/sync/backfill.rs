//! Stat-only backfill bucket. Fills in `book_files.(mtime_epoch,
//! size_bytes)` for rows still on the post-migration sentinel, without
//! touching metadata columns or rewriting link rows.

use sqlx::Transaction;

/// Apply the Backfill bucket as a single `UPDATE ... FROM (VALUES ..)`
/// per chunk. One statement per chunk instead of one per book so a
/// post-migration run over a large library doesn't hold the write lock
/// for thousands of individual writes (issue #245). Chunked to stay
/// well under SQLite's bound-parameter limit (3 binds per row, plus 1
/// for library_id).
pub(super) async fn backfill_stat_chunks(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    backfill: &[(String, i64, i64)],
) -> Result<(), sqlx::Error> {
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
