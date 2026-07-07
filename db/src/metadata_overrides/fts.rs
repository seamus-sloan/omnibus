//! Rebuild the `books_fts` row after an override write so search matches
//! the UI (merged canonical + override metadata). Wraps the
//! [`crate::sync::upsert_fts`] choke-point plus an [`overlay_overrides`]
//! patch in one transaction. Called best-effort by the upsert/merge paths;
//! the delete path restores canonical FTS in its own transaction instead.

use omnibus_shared::EbookMetadata;
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::books::{get_books_by_ids, resolve_book_id_by_uuid, resolve_book_ids_bulk};
use crate::sync::upsert_fts;

use super::upsert::MetadataOverridesError;

/// Rebuild the `books_fts` row for the book identified by `book_uuid`
/// using the merged metadata returned from [`crate::books::get_book`]
/// (canonical taxonomy with overrides applied). Called from the override
/// write paths so search matches what the UI displays.
///
/// Resolves the uuid through [`resolve_book_id_by_uuid`] so a merged /
/// attached uuid still lands on the surviving book. Silently returns
/// `Ok(())` if the uuid has no matching book — overrides for an unknown
/// uuid would only happen if a book row was deleted out from under us,
/// in which case there is no FTS row to maintain.
///
/// The canonical-write + override-overlay pair runs inside a single
/// transaction via [`rebuild_fts_row_tx`], so a failure in the overlay
/// rolls back the canonical write instead of leaving the FTS row with
/// unmerged data — matching [`rebuild_fts_for_books_batch`].
pub(crate) async fn rebuild_fts_for_book(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), MetadataOverridesError> {
    let Some(book_id) = resolve_book_id_by_uuid(pool, book_uuid).await? else {
        return Ok(());
    };
    let Some(merged) = crate::books::get_book(pool, book_id).await? else {
        return Ok(());
    };
    let mut tx = pool.begin().await?;
    rebuild_fts_row_tx(&mut tx, book_id, &merged).await?;
    tx.commit().await?;
    Ok(())
}

/// Write the canonical `books_fts` row then overlay the override-merged
/// columns, both against an open transaction so the pair commits (or rolls
/// back) atomically. The single write implementation shared by the
/// single-book ([`rebuild_fts_for_book`]) and batch
/// ([`rebuild_fts_for_books_batch`]) paths.
pub(crate) async fn rebuild_fts_row_tx(
    tx: &mut Transaction<'_, Sqlite>,
    book_id: i64,
    merged: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    upsert_fts(tx, book_id).await?;
    overlay_overrides(tx, book_id, merged).await?;
    Ok(())
}

/// Patch the override-driven columns of an existing `books_fts` row with
/// the merged metadata. The door already wrote the canonical row; this
/// overwrites `title / authors / series / tags / description` so search
/// reflects user edits. `isbn` is canonical-only (overrides can't change
/// identifiers), so it's left as the door wrote it.
async fn overlay_overrides(
    conn: &mut SqliteConnection,
    book_id: i64,
    merged: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let title = merged.title.clone().unwrap_or_default();
    let authors = crate::helpers::join_names(merged.creators.iter().map(|c| c.name.as_str()));
    let series = merged.series.clone().unwrap_or_default();
    let tags = crate::helpers::join_names(merged.subjects.iter().map(String::as_str));
    let description = merged.description.clone().unwrap_or_default();
    sqlx::query(
        "UPDATE books_fts
            SET title = ?, authors = ?, series = ?, tags = ?, description = ?
          WHERE rowid = ?",
    )
    .bind(&title)
    .bind(&authors)
    .bind(&series)
    .bind(&tags)
    .bind(&description)
    .bind(book_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Rebuild `books_fts` for all given UUIDs through the choke-point, with
/// override text overlaid. Each uuid is resolved via merged-uuids
/// fallback; uuids with no live book row are silently skipped.
///
/// Fetches merged metadata per-book before the write phase. Runs the
/// writes in one transaction so a large batch (e.g. an author delete
/// touching thousands of books) takes the write lock once.
pub(crate) async fn rebuild_fts_for_books_batch(
    pool: &SqlitePool,
    book_uuids: &[String],
) -> Result<(), MetadataOverridesError> {
    if book_uuids.is_empty() {
        return Ok(());
    }
    let rows = resolve_fts_rows(pool, book_uuids).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (book_id, merged) in &rows {
        rebuild_fts_row_tx(&mut tx, *book_id, merged).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Resolve UUIDs to `(book_id, merged metadata)`, skipping any uuid with
/// no live book row. Two chunked bulk queries regardless of batch size —
/// [`resolve_book_ids_bulk`] (uuid→id, with the merged/attached fallback)
/// then [`get_books_by_ids`] — joined in memory, replacing the former
/// two-query-per-uuid loop.
async fn resolve_fts_rows(
    pool: &SqlitePool,
    book_uuids: &[String],
) -> Result<Vec<(i64, EbookMetadata)>, MetadataOverridesError> {
    // De-dupe ids so a canonical uuid and one of its merged uuids appearing
    // together in the batch write the same FTS row only once.
    let id_map = resolve_book_ids_bulk(pool, book_uuids).await?;
    let mut ids: Vec<i64> = id_map.into_values().collect();
    ids.sort_unstable();
    ids.dedup();

    let books = get_books_by_ids(pool, &ids).await?;
    Ok(books.into_iter().map(|b| (b.id, b)).collect())
}
