//! Rebuild the `books_fts` row after an override write so search matches
//! what the UI displays (merged canonical + override metadata). Called
//! best-effort from the upsert/merge/delete paths.

use sqlx::SqlitePool;

use omnibus_shared::EbookMetadata;

use super::upsert::MetadataOverridesError;

/// Rebuild the `books_fts` row for the book identified by `book_uuid` using
/// the merged metadata returned from [`crate::books::get_book`] (canonical
/// taxonomy with overrides applied). Called from the override write paths
/// so search matches what the UI displays.
///
/// Silently returns `Ok(())` if the UUID has no matching book — overrides
/// for an unknown UUID would only happen if a book row was deleted out from
/// under us, in which case there is no FTS row to maintain.
pub(crate) async fn rebuild_fts_for_book(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), MetadataOverridesError> {
    let Some((book_id, merged)) = prepare_fts_row(pool, book_uuid).await? else {
        return Ok(());
    };
    let mut tx = pool.begin().await?;
    delete_stale_fts_row(&mut tx, book_id).await?;
    insert_fresh_fts_row(&mut tx, book_id, &merged).await?;
    tx.commit().await?;
    Ok(())
}

/// Resolve `books.uuid` → `(books.id, merged EbookMetadata)`. Returns
/// `None` if either the book row or its read-merge result is gone, so the
/// caller can no-op without a transaction.
async fn prepare_fts_row(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Option<(i64, EbookMetadata)>, MetadataOverridesError> {
    let Some(book_id) = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(book_uuid)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };
    let Some(merged) = crate::books::get_book(pool, book_id).await? else {
        return Ok(None);
    };
    Ok(Some((book_id, merged)))
}

/// Drop the existing `books_fts` row for `book_id` inside the rebuild
/// transaction so the subsequent insert can't see a stale duplicate.
async fn delete_stale_fts_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Materialize a fresh `books_fts` row from the merged metadata, mirroring
/// the canonical insert the sync path uses on scan.
async fn insert_fresh_fts_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    merged: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let title = merged.title.clone().unwrap_or_default();
    let first_isbn = merged
        .identifiers
        .iter()
        .find(|i| {
            i.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("ISBN"))
        })
        .map(|i| i.value.clone());
    crate::sync::insert_fts_row(tx, book_id, &title, first_isbn.as_deref(), merged).await
}
