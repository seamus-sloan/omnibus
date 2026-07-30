//! Removed bucket: mirrors `sync::books::removed` — retain each removed
//! audiobook's `books` row (metadata, links, FTS, soft-ref user data) and
//! drop only its own native `book_files` row(s) (parts/chapters cascade) —
//! a row still recorded in `merged_uuids` is a cross-format attachment and
//! survives — flagging it missing, unless that surviving attachment means
//! it isn't actually fileless, so the grid/facets hide it via their
//! `EXISTS book_files` filter. A returning group re-attaches via the
//! Changed bucket, preserving the uuid.

use sqlx::Transaction;

use super::super::attach;
use super::super::books::SyncError;

/// Apply the Removed bucket (F2): resolve affected ids and, via
/// `mark_book_files_missing_batch`, drop the `book_files` rows (parts/chapters
/// cascade) and flag each book missing — but **retain** the `books` row, its
/// links, FTS, and soft-ref user data, so the book stays in browse/search (the
/// grid hides it via `EXISTS book_files`) and a returning group re-attaches via
/// Changed. Also drop any `book_files` rows whose uuid lived only in
/// `merged_uuids` (cross-format attachments — the target book survives).
pub(super) async fn sync_audiobooks_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), SyncError> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    let mut missing = 0usize;
    // Chunk at 500 to stay under SQLite's 999-param cap when a whole library
    // (or any large diff) is removed — same convention as `sync_removed` in
    // `books/removed.rs`.
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        // Resolve affected ids once, then drop their file rows and flag them
        // missing with a single batched DELETE + UPDATE keyed on the id list —
        // retaining each `books` row (and its links/FTS) as a fileless book (F2).
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let ids = q.fetch_all(&mut **tx).await?;
        if ids.is_empty() {
            continue;
        }
        missing += ids.len();
        mark_book_files_missing_batch(tx, &ids).await?;
    }
    if missing > 0 {
        tracing::info!(
            missing,
            "sync: retained removed audiobooks as fileless books"
        );
    }

    // Removed uuids that were cross-format attachments have no
    // `books` row — drop their `book_files` row + `merged_uuids`
    // entry instead (the target book survives, possibly fileless).
    // `remove_attached_files` already chunks internally.
    attach::remove_attached_files(tx, removed_uuids).await?;
    Ok(())
}

/// Batched form of `books::mark_book_files_missing` for the Removed bucket: one
/// IN-list DELETE of the `book_files` rows (parts/chapters cascade) and one
/// guarded UPDATE flagging the now-fileless `books` rows missing (F2), instead
/// of two statements per book. The UPDATE keeps `mark_book_files_missing`'s
/// guards — `is_missing_files = 0` preserves the original `missing_files_since`
/// on a re-run, and `is_missing_files_override = 0` leaves intentionally-fileless
/// rows (wishlist) un-flagged. `ids` must be non-empty and within the SQLite
/// 999-param cap (the caller chunks at 500).
///
/// The DELETE excludes any row still recorded in `merged_uuids` — a
/// cross-format attachment (a different format's file, still present) —
/// so it survives this book's own group going missing. The UPDATE that
/// follows is guarded on `NOT EXISTS book_files` too, so a book whose
/// cross-format attachment survived the delete (and so isn't actually
/// fileless) is not flagged missing.
async fn mark_book_files_missing_batch(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
) -> Result<(), SyncError> {
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(", ");

    let del_sql = format!(
        "DELETE FROM book_files
          WHERE book_id IN ({placeholders})
            AND NOT EXISTS (
              SELECT 1 FROM merged_uuids mu
               WHERE mu.book_id = book_files.book_id
                 AND mu.format = book_files.format
                 AND mu.scan_key = book_files.scan_key
            )"
    );
    let mut del_q = sqlx::query(&del_sql);
    for id in ids {
        del_q = del_q.bind(id);
    }
    del_q.execute(&mut **tx).await?;

    let upd_sql = format!(
        "UPDATE books
            SET is_missing_files = 1, missing_files_since = unixepoch()
          WHERE id IN ({placeholders})
            AND is_missing_files = 0
            AND is_missing_files_override = 0
            AND NOT EXISTS (SELECT 1 FROM book_files WHERE book_files.book_id = books.id)"
    );
    let mut upd_q = sqlx::query(&upd_sql);
    for id in ids {
        upd_q = upd_q.bind(id);
    }
    upd_q.execute(&mut **tx).await?;
    Ok(())
}
