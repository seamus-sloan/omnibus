//! Rebuild the `books_fts` row after an override write so search matches
//! what the UI displays (merged canonical + override metadata). Thin
//! wrappers over the [`crate::sync::upsert_fts`] choke-point: the door
//! writes the canonical row, then [`overlay_overrides`] patches the
//! overridable columns with the merged values. Called best-effort from
//! the upsert/merge/delete paths.

use sqlx::{SqliteConnection, SqlitePool};

use omnibus_shared::EbookMetadata;

use crate::books::resolve_book_id_by_uuid;
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
/// no live book row. Uses [`resolve_book_id_by_uuid`] so merged/attached
/// uuids resolve to the surviving book.
async fn resolve_fts_rows(
    pool: &SqlitePool,
    book_uuids: &[String],
) -> Result<Vec<(i64, EbookMetadata)>, MetadataOverridesError> {
    let mut rows: Vec<(i64, EbookMetadata)> = Vec::with_capacity(book_uuids.len());
    for uuid in book_uuids {
        let Some(book_id) = resolve_book_id_by_uuid(pool, uuid).await? else {
            continue;
        };
        let Some(merged) = crate::books::get_book(pool, book_id).await? else {
            continue;
        };
        rows.push((book_id, merged));
    }
    Ok(rows)
}
