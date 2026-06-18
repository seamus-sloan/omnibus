//! Tail of the transaction: stamp `scan_roots.last_indexed`. The
//! stat-only `book_files` UPDATE that pairs with this step lives in
//! `super::super::backfill::backfill_stat_chunks` and is invoked
//! directly from `sync_books_with_progress` (path: `sync::backfill`).

use sqlx::Transaction;

/// Stamp `scan_roots.last_indexed` with the current wall-clock seconds.
pub(super) async fn stamp_last_indexed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
) -> Result<(), sqlx::Error> {
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
