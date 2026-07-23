//! The SQL half of deletion: dropping `book_files` / `physical_copies` rows,
//! and the full purge of a book row plus everything soft-referencing its uuid.
//! Mirrors `missing_files::delete_victim_rows` (the GC's hard delete) but
//! keeps nothing back — a deliberate delete takes the user data with it,
//! because the confirmation dialog already named it.

use sqlx::{SqlitePool, Transaction};

use super::DeleteError;

/// Every table that soft-references `books.uuid` (no FK, no cascade — see
/// migration 0027), so a book purge can't leave a row keyed to a uuid that
/// will never resolve again. `metadata_overrides` and the suggestion cache
/// ride along: both are regenerable, and both key on the same uuid.
const UUID_KEYED_TABLES: &[&str] = &[
    "metadata_overrides",
    "reading_progress",
    "bookmarks",
    "reading_sessions",
    "listening_sessions",
    "highlights",
    "user_ratings",
    "audiobook_playback_preferences",
    "journal_entries",
    "shelf_books",
    "book_suggestions",
    "book_suggestion_state",
    "physical_copies",
    "wishlist_entries",
];

/// Delete the given `book_files` rows (parts and chapters cascade via their
/// FK) along with any `merged_uuids` guard the files held.
///
/// The ledger rows go first: they join on `(book_id, scan_key)`, so once the
/// file rows are gone there's nothing left to match them by, and a stale guard
/// would keep replaying the attach on every future scan.
pub(super) async fn delete_file_rows(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    file_ids: &[i64],
) -> Result<(), DeleteError> {
    if file_ids.is_empty() {
        return Ok(());
    }
    // Chunked at 500 to stay under SQLite's 999-bind cap, matching the
    // convention in `sync::attach::remove_attached_files`.
    for chunk in file_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        let ledger_sql = format!(
            "DELETE FROM merged_uuids
              WHERE book_id = ?
                AND scan_key IN (SELECT scan_key FROM book_files
                                  WHERE id IN ({placeholders}) AND scan_key IS NOT NULL)"
        );
        let mut q = sqlx::query(&ledger_sql).bind(book_id);
        for id in chunk {
            q = q.bind(id);
        }
        q.execute(&mut **tx).await?;

        let files_sql =
            format!("DELETE FROM book_files WHERE book_id = ? AND id IN ({placeholders})");
        let mut q = sqlx::query(&files_sql).bind(book_id);
        for id in chunk {
            q = q.bind(id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Un-record the given physical copies. Scoped to the book's uuid so an id
/// from another book can't be deleted through this door (the caller has
/// already validated ownership; this is the second gate).
pub(super) async fn delete_copy_rows(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    uuid: &str,
    copy_ids: &[i64],
) -> Result<(), DeleteError> {
    if copy_ids.is_empty() {
        return Ok(());
    }
    for chunk in copy_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("DELETE FROM physical_copies WHERE book_uuid = ? AND id IN ({placeholders})");
        let mut q = sqlx::query(&sql).bind(uuid);
        for id in chunk {
            q = q.bind(id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Hard-delete the book: every uuid-keyed row, any remaining `merged_uuids`
/// guard, the `books` row, its search twin, and any taxonomy the delete
/// orphaned. Link tables and identifiers cascade from `books`.
pub(super) async fn purge_book(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    uuid: &str,
) -> Result<(), DeleteError> {
    for table in UUID_KEYED_TABLES {
        // Table names come from the constant above, never from caller input.
        let sql = format!("DELETE FROM {table} WHERE book_uuid = ?");
        sqlx::query(&sql).bind(uuid).execute(&mut **tx).await?;
    }
    sqlx::query("DELETE FROM merged_uuids WHERE book_id = ?")
        .bind(book_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(book_id)
        .execute(&mut **tx)
        .await?;
    // `books_fts` is standalone (no `content=`), so its row only leaves through
    // this door — same as the GC purge and the merge source delete.
    crate::sync::delete_fts(tx, book_id).await?;
    crate::taxonomy::delete_orphan_taxonomy(tx).await?;
    Ok(())
}

/// Journal-image file names embedded in this book's entries, collected before
/// the purge drops the bodies that reference them. Candidates only — run
/// [`orphaned_images`] after the commit to drop any another book still uses.
pub(super) async fn journal_image_names(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Vec<String>, DeleteError> {
    let bodies: Vec<String> =
        sqlx::query_scalar("SELECT body_md FROM journal_entries WHERE book_uuid = ?")
            .bind(uuid)
            .fetch_all(pool)
            .await?;
    let mut names: Vec<String> = bodies
        .iter()
        .flat_map(|body| crate::journal_images::referenced_image_names(body))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Narrow `candidates` to the names no surviving `journal_entries.body_md`
/// still embeds. Matched on the full embed path rather than the bare name for
/// the same reason as `journals::cleanup_orphaned_images`: a bare-name
/// substring can false-positive on unrelated body text.
pub(super) async fn orphaned_images(
    pool: &SqlitePool,
    candidates: Vec<String>,
) -> Result<Vec<String>, DeleteError> {
    let mut orphaned = Vec::new();
    for name in candidates {
        let embed = format!("{}{name}", crate::journals::markdown::IMAGE_URL_PREFIX);
        let still_referenced: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM journal_entries WHERE instr(body_md, ?) > 0)",
        )
        .bind(&embed)
        .fetch_one(pool)
        .await?;
        if !still_referenced {
            orphaned.push(name);
        }
    }
    Ok(orphaned)
}
