//! Deliberate book/file deletion, driven by the admin delete dialog — the
//! counterpart to the scanner's *ghosting*, which retains the `books` row.
//! The unit of deletion is an *item*: a `book_files` row (also unlinked from
//! disk) or a physical copy (un-recorded only). The `books` row goes only
//! when every item does. Split into [`purge`] (SQL) and [`fs`] (filesystem).

mod fs;
mod purge;

#[cfg(test)]
mod tests;

use sqlx::SqlitePool;

use omnibus_shared::physical::PhysicalCopy;
use omnibus_shared::BookFileInfo;

use crate::books::{book_file_path_by_id, get_book_files, resolve_book_id_by_uuid};

/// Errors from the deletion path.
#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    /// No book resolves from the given uuid (honoring `merged_uuids`).
    #[error("book not found")]
    BookNotFound,
    /// A requested `book_files.id` doesn't belong to the resolved book.
    #[error("book file {0} not found on this book")]
    FileNotFound(i64),
    /// A requested `physical_copies.id` doesn't belong to the resolved book.
    #[error("physical copy {0} not found on this book")]
    CopyNotFound(i64),
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
    #[error(transparent)]
    Physical(#[from] crate::physical::PhysicalError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Everything the delete dialog lists for one book: its deletable items, plus
/// the user data a total delete would take with them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeletionManifest {
    pub files: Vec<BookFileInfo>,
    pub copies: Vec<PhysicalCopy>,
    pub impact: DeletionImpact,
}

/// Counts of the uuid-keyed user data a total delete would purge.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeletionImpact {
    pub highlights: i64,
    pub journal_entries: i64,
    pub bookmarks: i64,
    pub reading_sessions: i64,
    pub listening_sessions: i64,
    pub ratings: i64,
    pub shelves: i64,
}

/// What a [`delete_book_items`] call actually removed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub deleted_file_ids: Vec<i64>,
    pub deleted_copy_ids: Vec<i64>,
    /// True when the book row itself was purged (every item went, or it had
    /// none to begin with).
    pub book_deleted: bool,
}

/// Read the book's deletable items and the user data attached to it.
pub async fn book_deletion_manifest(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<DeletionManifest, DeleteError> {
    let book_id = resolve_book_id_by_uuid(pool, book_uuid)
        .await?
        .ok_or(DeleteError::BookNotFound)?;
    let uuid = canonical_uuid(pool, book_id).await?;

    let mut impact = DeletionImpact::default();
    for (table, slot) in [
        ("highlights", &mut impact.highlights),
        ("journal_entries", &mut impact.journal_entries),
        ("bookmarks", &mut impact.bookmarks),
        ("reading_sessions", &mut impact.reading_sessions),
        ("listening_sessions", &mut impact.listening_sessions),
        ("user_ratings", &mut impact.ratings),
        ("shelf_books", &mut impact.shelves),
    ] {
        *slot = count_by_uuid(pool, table, &uuid).await?;
    }

    Ok(DeletionManifest {
        files: get_book_files(pool, book_id).await?,
        copies: crate::physical::list_physical_copies(pool, &uuid).await?,
        impact,
    })
}

/// Delete the given items: `file_ids` are `book_files` rows (removed from the
/// DB and unlinked from disk) and `copy_ids` are physical copies (un-recorded
/// only — nothing on disk to remove). When no item is left afterwards, the
/// `books` row and everything keyed to its uuid go too (see [`purge`]).
///
/// The DB write commits first and the filesystem cleanup follows best-effort:
/// a failed unlink leaves an orphan file the next scan re-indexes as a new
/// book, which is recoverable, whereas a deleted file with its row intact is
/// the ghost state this exists to avoid. By the same token a scan already in
/// flight can re-insert a file it snapshotted before the unlink (as a fresh
/// book with a new uuid); the next scan won't, so that's left to resolve
/// itself rather than locking against the worker.
pub async fn delete_book_items(
    pool: &SqlitePool,
    book_uuid: &str,
    file_ids: &[i64],
    copy_ids: &[i64],
) -> Result<DeleteOutcome, DeleteError> {
    let book_id = resolve_book_id_by_uuid(pool, book_uuid)
        .await?
        .ok_or(DeleteError::BookNotFound)?;
    let uuid = canonical_uuid(pool, book_id).await?;

    // De-duplicated up front: `total_delete` is decided by comparing the
    // selection's length against the book's item count, so a retry or client
    // bug that repeats an id would inflate the count and skip the purge even
    // though every item was picked.
    let file_ids = &dedup(file_ids);
    let copy_ids = &dedup(copy_ids);

    let existing_files = get_book_files(pool, book_id).await?;
    if let Some(unknown) = file_ids
        .iter()
        .find(|id| !existing_files.iter().any(|f| f.id == **id))
    {
        return Err(DeleteError::FileNotFound(*unknown));
    }
    let existing_copies = crate::physical::list_physical_copies(pool, &uuid).await?;
    if let Some(unknown) = copy_ids
        .iter()
        .find(|id| !existing_copies.iter().any(|c| c.id == **id))
    {
        return Err(DeleteError::CopyNotFound(*unknown));
    }

    let item_count = existing_files.len() + existing_copies.len();
    let selected = file_ids.len() + copy_ids.len();
    // Selecting nothing on a book that still has items is a no-op; the same
    // call on a book with no items at all is the "delete this record" case.
    if selected == 0 && item_count > 0 {
        return Ok(DeleteOutcome::default());
    }
    let total_delete = selected == item_count;

    // Resolve on-disk paths before the rows they're derived from disappear.
    let mut paths = Vec::with_capacity(file_ids.len());
    for id in file_ids {
        if let Some(path) = book_file_path_by_id(pool, book_id, *id, None).await? {
            paths.push(path);
        }
    }
    let library_roots = library_roots(pool).await?;
    let journal_images = if total_delete {
        purge::journal_image_names(pool, &uuid).await?
    } else {
        Vec::new()
    };

    let mut tx = pool.begin().await?;
    purge::delete_file_rows(&mut tx, book_id, file_ids).await?;
    purge::delete_copy_rows(&mut tx, &uuid, copy_ids).await?;
    if total_delete {
        purge::purge_book(&mut tx, book_id, &uuid).await?;
    }
    tx.commit().await?;

    // Post-commit, so an image another book's entry still embeds survives.
    let journal_images = purge::orphaned_images(pool, journal_images).await?;

    fs::cleanup(fs::Cleanup {
        paths,
        library_roots,
        book: total_delete.then_some(fs::BookCleanup {
            book_id,
            uuid,
            journal_images,
        }),
    })
    .await;

    Ok(DeleteOutcome {
        deleted_file_ids: file_ids.to_vec(),
        deleted_copy_ids: copy_ids.to_vec(),
        book_deleted: total_delete,
    })
}

/// Copy of `ids` with duplicates removed, order-independent (the caller only
/// counts and binds them).
fn dedup(ids: &[i64]) -> Vec<i64> {
    let mut out = ids.to_vec();
    out.sort_unstable();
    out.dedup();
    out
}

/// The `books.uuid` for `book_id` — the passed-in uuid may be a `merged_uuids`
/// ledger key, and every user-data table keys on the canonical one.
async fn canonical_uuid(pool: &SqlitePool, book_id: i64) -> Result<String, DeleteError> {
    sqlx::query_scalar::<_, String>("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await?
        .ok_or(DeleteError::BookNotFound)
}

/// Every configured scan root, used as the stop line for the empty-directory
/// prune so cleanup can never walk out of a library.
async fn library_roots(pool: &SqlitePool) -> Result<Vec<String>, DeleteError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT path FROM scan_roots")
            .fetch_all(pool)
            .await?,
    )
}

/// `SELECT COUNT(*)` for a `book_uuid`-keyed table. `table` is always one of
/// the compile-time literals above, never caller input.
async fn count_by_uuid(pool: &SqlitePool, table: &str, uuid: &str) -> Result<i64, DeleteError> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE book_uuid = ?");
    Ok(sqlx::query_scalar(&sql).bind(uuid).fetch_one(pool).await?)
}
