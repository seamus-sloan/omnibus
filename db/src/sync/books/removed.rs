//! Removed bucket: every uuid whose file disappeared keeps its `books` row.
//! `mark_book_files_missing` drops only the `book_files` row and flags the book
//! missing for F10 GC — its metadata, taxonomy links, and FTS row stay, so the
//! book remains in browse/search; the grid and facets hide it via their own
//! `EXISTS book_files` filter. Idempotent — a re-run on an already-fileless row
//! deletes zero `book_files`.

use sqlx::Transaction;

use super::shared::mark_book_files_missing;

/// Mark a batch of removed books' files missing (F2). The file is gone, but the
/// `books` row — its metadata, taxonomy links, FTS row, and every soft-ref
/// user-data row — is **retained** (only `book_files` is dropped, parts and
/// chapters cascading), so the book stays in author/series/tag browse and
/// search while the grid/facets hide it via their `EXISTS book_files` filter. A
/// returning file re-attaches via the Changed path, preserving the uuid.
/// Idempotent: a re-run on an already-fileless row deletes zero `book_files`.
pub(super) async fn sync_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), sqlx::Error> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    let mut missing = 0usize;
    // library_id + 1 bind per uuid; chunk at 500 to stay under SQLite's
    // 999-param cap when a whole library (or any large diff) is removed.
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        // Resolve the affected ids, then drop their file rows — keeping each
        // `books` row (and its links/FTS) as a fileless book.
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let ids = q.fetch_all(&mut **tx).await?;
        missing += ids.len();
        for id in ids {
            mark_book_files_missing(tx, id).await?;
        }
    }
    if missing > 0 {
        // These rows are flagged missing (F10); `missing_files::gc_books_missing_files`
        // purges the long-missing, user-data-free ones on a later reindex.
        tracing::info!(missing, "sync: retained removed books as fileless books");
    }
    Ok(())
}
