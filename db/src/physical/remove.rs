//! Hard-delete a fileless book. The last physical copy of a book with no
//! digital files can be dropped ("I sold it"), which would otherwise leave a
//! row visible nowhere — book detail offers "remove it entirely" instead.

use sqlx::SqlitePool;

use super::PhysicalError;
use crate::books::resolve_canonical_book_uuid;
use crate::covers::delete_cover_files_for;
use crate::metadata_overrides::delete_override_cover;

/// Hard-delete a book that has no `book_files`, taking **every** uuid-keyed row
/// with it (ratings, shelves, progress, journals, …) via the canonical
/// [`purge_book`] sweep — not just the physical/wishlist/override rows — plus
/// the `books` row, its FTS twin, orphan taxonomy, and cover files.
///
/// Refuses a file-backed book with [`PhysicalError::BookHasFiles`]: those are
/// owned by the reindex diff, and deleting one here would resurrect it on the
/// next scan while destroying user data keyed on its uuid. A caller that wants
/// a file-backed book gone removes the file and lets the ghosting path run.
///
/// Cover unlink is best-effort and runs after the commit — covers are a
/// rebuildable cache, so a failed unlink must not fail the delete (mirrors
/// `gc_books_missing_files`).
pub async fn delete_fileless_book(pool: &SqlitePool, book_uuid: &str) -> Result<(), PhysicalError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(PhysicalError::BookNotFound)?;

    let mut tx = pool.begin().await?;

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(&canonical)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(PhysicalError::BookNotFound)?;

    let file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?1")
        .bind(book_id)
        .fetch_one(&mut *tx)
        .await?;
    if file_count > 0 {
        return Err(PhysicalError::BookHasFiles);
    }

    // One canonical sweep of every uuid-keyed table + the books row + FTS twin +
    // orphan taxonomy, shared with the admin delete path.
    crate::deletion::purge::purge_book(&mut tx, book_id, &canonical)
        .await
        .map_err(map_delete_error)?;
    tx.commit().await?;

    let uuids = vec![canonical];
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        delete_override_cover(&uuids[0]);
        delete_cover_files_for(&uuids);
    })
    .await
    {
        tracing::error!(error = %join_err, "delete_fileless_book: cover cleanup spawn_blocking failed");
    }
    Ok(())
}

/// Fold a `deletion::DeleteError` from the shared purge into `PhysicalError`.
/// `purge_book` only ever emits `Sqlx` here (the book is already resolved and
/// fileless), so the remaining variants collapse defensively onto `Other`.
pub(super) fn map_delete_error(e: crate::deletion::DeleteError) -> PhysicalError {
    match e {
        crate::deletion::DeleteError::Sqlx(inner) => PhysicalError::Sqlx(inner),
        crate::deletion::DeleteError::Physical(inner) => inner,
        crate::deletion::DeleteError::Books(inner) => PhysicalError::Books(inner),
        other => PhysicalError::Other(other.to_string()),
    }
}
