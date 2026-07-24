//! Hard-delete a fileless book. The last physical copy of a book with no
//! digital files can be dropped ("I sold it"), which would otherwise leave a
//! row visible nowhere — book detail offers "remove it entirely" instead.

use sqlx::SqlitePool;

use super::PhysicalError;
use crate::books::resolve_canonical_book_uuid;
use crate::covers::delete_cover_files_for;
use crate::metadata_overrides::delete_override_cover;

/// Hard-delete a book that has no `book_files`, along with its physical copies,
/// wishlist entries, overrides, FTS twin, and cover files.
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

    // Soft-ref user data (no FK, no cascade) has to go by hand; the `books`
    // delete cascades the link tables on its own.
    for sql in [
        "DELETE FROM physical_copies    WHERE book_uuid = ?1",
        "DELETE FROM wishlist_entries   WHERE book_uuid = ?1",
        "DELETE FROM metadata_overrides WHERE book_uuid = ?1",
    ] {
        sqlx::query(sql).bind(&canonical).execute(&mut *tx).await?;
    }

    sqlx::query("DELETE FROM books WHERE id = ?1")
        .bind(book_id)
        .execute(&mut *tx)
        .await?;
    // `books_fts` has no FK to `books`, so its twin (rowid = book id) only
    // leaves the index here.
    sqlx::query("DELETE FROM books_fts WHERE rowid = ?1")
        .bind(book_id)
        .execute(&mut *tx)
        .await?;

    crate::taxonomy::delete_orphan_taxonomy(&mut tx).await?;
    tx.commit().await?;

    let uuids = vec![canonical];
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        delete_override_cover(&uuids[0]);
        delete_cover_files_for(&uuids);
    })
    .await
    {
        tracing::error!("delete_fileless_book: cover cleanup spawn_blocking failed: {join_err}");
    }
    Ok(())
}
