//! Rebuild the `books_fts` row after an override write so search matches
//! what the UI displays (merged canonical + override metadata). Thin
//! wrappers over the [`crate::sync::upsert_fts`] choke-point: the door
//! writes the canonical row, then [`overlay_overrides`] patches the
//! overridable columns with the merged values. Called best-effort from
//! the upsert/merge/delete paths.

use sqlx::{SqliteConnection, SqlitePool};

use omnibus_shared::EbookMetadata;

use crate::books::{get_books_by_ids, resolve_book_id_by_uuid, resolve_book_ids_by_uuids};
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
    let mut conn = pool.acquire().await?;
    upsert_fts(&mut conn, book_id).await?;
    overlay_overrides(&mut conn, book_id, &merged).await?;
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
        upsert_fts(&mut tx, *book_id).await?;
        overlay_overrides(&mut tx, *book_id, merged).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Resolve UUIDs to `(book_id, merged metadata)`, skipping any uuid with
/// no live book row. Uses [`resolve_book_ids_by_uuids`] so merged/attached
/// uuids resolve to the surviving book.
///
/// Two round-trips per chunk (chunked at 499 to respect SQLite's
/// bind-parameter cap): one bulk uuid→id resolve, then one bulk book
/// fetch. A per-uuid loop would issue 2N round-trips, which dominates
/// the write phase for the large batches this path fires on
/// (author-photo updates, multi-book override saves).
///
/// The two lookups are joined in memory; uuids that resolve to an id
/// whose book row has since vanished are silently dropped (same
/// tolerance the pre-batch [`rebuild_fts_for_book`] path applied).
async fn resolve_fts_rows(
    pool: &SqlitePool,
    book_uuids: &[String],
) -> Result<Vec<(i64, EbookMetadata)>, MetadataOverridesError> {
    let id_by_uuid = resolve_book_ids_by_uuids(pool, book_uuids).await?;
    if id_by_uuid.is_empty() {
        return Ok(Vec::new());
    }
    // Deduplicate: two uuids can point at the same book (a merged/attached
    // ledger key alongside its target's own uuid), and we only need to
    // rebuild each FTS row once.
    let mut ids: Vec<i64> = id_by_uuid.values().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut book_by_id = get_books_by_ids(pool, &ids).await?;
    let mut rows: Vec<(i64, EbookMetadata)> = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(merged) = book_by_id.remove(&id) {
            rows.push((id, merged));
        }
    }
    Ok(rows)
}
