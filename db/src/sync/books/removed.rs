//! Removed bucket: ghost every uuid whose file disappeared. The `books`
//! row stays (preserving uuid + soft-ref user data); only `book_files`,
//! per-book link rows, and the FTS row are dropped. Idempotent — a
//! re-ghost on an already-fileless row is a no-op write-wise.

use sqlx::Transaction;

use super::shared::ghost_book_by_id;

/// Ghost a batch of removed books by uuid (F2). The file is gone, but the
/// `books` row — and every soft-ref user-data row keyed on `books.uuid` —
/// is **retained**: drop the `book_files` row (parts/chapters cascade) and
/// clear the `books_fts` row (FTS5 is standalone — no FK) so the now-fileless
/// ghost is hidden from search, then leave the `books` row in place. A
/// returning file re-attaches via the Changed path, preserving the uuid.
/// Idempotent: re-ghosting an already-fileless row deletes zero `book_files`.
pub(super) async fn sync_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), sqlx::Error> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    let mut ghosted = 0usize;
    // library_id + 1 bind per uuid; chunk at 500 to stay under SQLite's
    // 999-param cap when a whole library (or any large diff) is removed.
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        // Resolve the affected ids, clear each FTS row via the door, then
        // drop the file rows — keeping the `books` row as a fileless ghost.
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let ids = q.fetch_all(&mut **tx).await?;
        ghosted += ids.len();
        for id in ids {
            ghost_book_by_id(tx, id).await?;
        }
    }
    if ghosted > 0 {
        // GC of long-fileless ghosts is deferred to F10; log the count so
        // accumulation is observable until then.
        tracing::info!(ghosted, "sync: retained removed books as fileless ghosts");
    }
    Ok(())
}
