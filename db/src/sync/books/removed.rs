//! Removed bucket: every uuid whose file disappeared keeps its `books` row.
//! One chunked DELETE drops only the `book_files` rows, and one chunked UPDATE
//! flags each book missing for F10 GC — its metadata, taxonomy links, and FTS
//! row stay, so the book remains in browse/search; the grid and facets hide it
//! via their own `EXISTS book_files` filter. Idempotent — a re-run on an
//! already-fileless row deletes zero `book_files` and the flag UPDATE no-ops
//! (the `is_missing_files = 0` guard preserves the original
//! `missing_files_since`).

use sqlx::Transaction;

/// Mark a batch of removed books' files missing (F2). The file is gone, but the
/// `books` row — its metadata, taxonomy links, FTS row, and every soft-ref
/// user-data row — is **retained** (only `book_files` is dropped, parts and
/// chapters cascading), so the book stays in author/series/tag browse and
/// search while the grid/facets hide it via their `EXISTS book_files` filter. A
/// returning file re-attaches via the Changed path, preserving the uuid.
/// Idempotent: a re-run on an already-fileless row deletes zero `book_files`
/// and the flag UPDATE is a no-op via the `is_missing_files = 0` guard.
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

        // Resolve the affected ids, then batch-drop their file rows and flag
        // the rows missing — keeping each `books` row (and its links/FTS) as a
        // fileless book. Two chunked statements replace the old per-book
        // `mark_book_files_missing` fan-out (which fired 2 DML per removed
        // book).
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let ids = q.fetch_all(&mut **tx).await?;
        missing += ids.len();
        if ids.is_empty() {
            continue;
        }

        let id_placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");

        // One DELETE per chunk: `book_file_parts` + `file_chapters` cascade
        // off the `book_files` row, so a single sweep drops everything the
        // file owned.
        let delete_sql = format!("DELETE FROM book_files WHERE book_id IN ({id_placeholders})");
        let mut delete_q = sqlx::query(&delete_sql);
        for id in &ids {
            delete_q = delete_q.bind(id);
        }
        delete_q.execute(&mut **tx).await?;

        // One UPDATE per chunk: set the F10 missing flag + start the retention
        // clock. The `is_missing_files = 0` guard preserves the original
        // `missing_files_since` on a re-run; the `is_missing_files_override = 0`
        // guard leaves intentionally-fileless rows (wishlist) unflagged so
        // reads keep showing them.
        let update_sql = format!(
            "UPDATE books
                SET is_missing_files = 1, missing_files_since = unixepoch()
              WHERE id IN ({id_placeholders})
                AND is_missing_files = 0
                AND is_missing_files_override = 0"
        );
        let mut update_q = sqlx::query(&update_sql);
        for id in &ids {
            update_q = update_q.bind(id);
        }
        update_q.execute(&mut **tx).await?;
    }
    if missing > 0 {
        // These rows are flagged missing (F10); `missing_files::gc_books_missing_files`
        // purges the long-missing, user-data-free ones on a later reindex.
        tracing::info!(missing, "sync: retained removed books as fileless books");
    }
    Ok(())
}
