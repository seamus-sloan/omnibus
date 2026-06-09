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

/// Rebuild `books_fts` for all given UUIDs: one batched DELETE + INSERT per chunk of 100.
///
/// Fetches merged metadata per-book (N reads, no write lock) before the write phase.
/// UUIDs with no matching `books` row are silently skipped.
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
    write_fts_chunks(pool, &rows).await
}

struct FtsRow {
    book_id: i64,
    title: String,
    authors: String,
    series: String,
    tags: String,
    description: String,
    isbn: String,
}

/// Resolve UUIDs to merged FTS data, skipping UUIDs with no live book row.
async fn resolve_fts_rows(
    pool: &SqlitePool,
    book_uuids: &[String],
) -> Result<Vec<FtsRow>, MetadataOverridesError> {
    let mut rows: Vec<FtsRow> = Vec::with_capacity(book_uuids.len());
    for uuid in book_uuids {
        let Some(book_id) = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
            .bind(uuid)
            .fetch_optional(pool)
            .await?
        else {
            continue;
        };
        let Some(merged) = crate::books::get_book(pool, book_id).await? else {
            continue;
        };

        let title = merged.title.clone().unwrap_or_default();
        let isbn = merged
            .identifiers
            .iter()
            .find(|i| {
                i.scheme
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("ISBN"))
            })
            .map(|i| i.value.clone())
            .unwrap_or_default();
        let authors = crate::helpers::join_names(merged.creators.iter().map(|c| c.name.as_str()));
        let series = merged.series.clone().unwrap_or_default();
        let tags = crate::helpers::join_names(merged.subjects.iter().map(String::as_str));
        let description = merged.description.clone().unwrap_or_default();

        rows.push(FtsRow {
            book_id,
            title,
            authors,
            series,
            tags,
            description,
            isbn,
        });
    }
    Ok(rows)
}

/// Write FTS rows in chunks of 100: one DELETE + one multi-row INSERT per chunk.
async fn write_fts_chunks(
    pool: &SqlitePool,
    rows: &[FtsRow],
) -> Result<(), MetadataOverridesError> {
    let mut tx = pool.begin().await?;
    for chunk in rows.chunks(100) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let delete_sql = format!("DELETE FROM books_fts WHERE rowid IN ({placeholders})");
        let mut q = sqlx::query(&delete_sql);
        for row in chunk {
            q = q.bind(row.book_id);
        }
        q.execute(&mut *tx).await?;

        let value_placeholders = std::iter::repeat_n("(?, ?, ?, ?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let insert_sql = format!(
            "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn) \
             VALUES {value_placeholders}"
        );
        let mut q = sqlx::query(&insert_sql);
        for row in chunk {
            q = q
                .bind(row.book_id)
                .bind(&row.title)
                .bind(&row.authors)
                .bind(&row.series)
                .bind(&row.tags)
                .bind(&row.description)
                .bind(&row.isbn);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
