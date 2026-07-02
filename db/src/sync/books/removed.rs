//! Removed bucket: every uuid whose file disappeared keeps its `books` row.
//! Only the `book_files` row is dropped and the book is flagged missing for F10
//! GC — its metadata, taxonomy links, and FTS row stay, so the book remains in
//! browse/search; the grid and facets hide it via their own `EXISTS book_files`
//! filter. Idempotent — a re-run on an already-fileless row deletes zero
//! `book_files`.

use sqlx::Transaction;

use super::shared::mark_book_files_missing_batch;

/// Mark a batch of removed books' files missing (F2). The file is gone, but the
/// `books` row — its metadata, taxonomy links, FTS row, and every soft-ref
/// user-data row — is **retained** (only `book_files` is dropped, parts and
/// chapters cascading), so the book stays in author/series/tag browse and
/// search while the grid/facets hide it via their `EXISTS book_files` filter. A
/// returning file re-attaches via the Changed path, preserving the uuid.
/// Idempotent: a re-run on an already-fileless row deletes zero `book_files`.
///
/// Resolves every affected `books.id` in chunked `uuid IN (...)` SELECTs, then
/// drops their file rows + flags them missing in batched DML — no per-book
/// fan-out (mirrors the Changed bucket's batched id lookup).
pub(super) async fn sync_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), sqlx::Error> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    // library_id + 1 bind per uuid; chunk at 500 to stay under SQLite's
    // 999-param cap when a whole library (or any large diff) is removed.
    let mut ids: Vec<i64> = Vec::new();
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        ids.extend(q.fetch_all(&mut **tx).await?);
    }

    mark_book_files_missing_batch(tx, &ids).await?;

    if !ids.is_empty() {
        // These rows are flagged missing (F10); `missing_files::gc_books_missing_files`
        // purges the long-missing, user-data-free ones on a later reindex.
        tracing::info!(
            missing = ids.len(),
            "sync: retained removed books as fileless books"
        );
    }
    Ok(())
}
