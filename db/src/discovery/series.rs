//! Series-detail read: a single series plus its books, ordered by the
//! override-aware series index. A single-pass `effective` CTE merges the
//! canonical `books_series_link` membership with the form-edited override
//! set so books moved into or out of a series via the edit form surface
//! here too.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{EbookMetadata, SeriesDetail};

use crate::books::{backfill_creator_ids, row_to_ebook, BOOK_COLUMNS};
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk};

use super::{DiscoveryError, MAX_DISCOVERY_BOOKS};

/// Fetch a series by ID with its books, ordered by series index. Returns
/// `None` if the series ID doesn't exist. The nested `books` vec is
/// capped at [`MAX_DISCOVERY_BOOKS`]; `book_count` is uncapped.
pub async fn get_series(
    pool: &SqlitePool,
    series_id: i64,
) -> Result<Option<SeriesDetail>, DiscoveryError> {
    // TODO: scope by `user_id` once per-user ACLs land (single-tenant today).
    let series_row = sqlx::query("SELECT id, name, sort FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_optional(pool)
        .await?;
    let Some(s) = series_row else {
        return Ok(None);
    };
    let series_name: String = s.get("name");

    let mut books = fetch_series_books(pool, series_id, &series_name).await?;
    let book_count = count_effective_series_members(pool, series_id, &series_name).await?;
    merge_and_pin_series(pool, &mut books, series_id, &series_name).await?;
    backfill_creator_ids(pool, &mut books).await?;

    Ok(Some(SeriesDetail {
        id: s.get("id"),
        name: s.get("name"),
        sort: s.get("sort"),
        book_count: usize::try_from(book_count).unwrap_or(0),
        books,
    }))
}

/// SQL CTE expressing the override-aware effective series membership. Used
/// by both `fetch_series_books` (with the full `BOOK_COLUMNS` SELECT) and
/// `count_effective_series_members` (with `COUNT(*)`).
///
/// Single-pass UNION over (1) canonical link rows for *this* series whose
/// book has no `series` override, and (2) override rows whose
/// `overrides.series` matches this series' name. Both arms are scoped to
/// the target series up front, so arm (1) drives through the
/// `books_series_link(series)` index instead of scanning every book.
///
/// The empty-string clear-all case is handled naturally: a `Some("")`
/// override removes the book from arm (1) (override IS NOT NULL) and the
/// `'' = name` filter excludes it from arm (2). Case-insensitivity is
/// preserved by the `COLLATE NOCASE` on arm (2)'s name comparison.
const EFFECTIVE_SERIES_CTE: &str = r#"WITH effective AS (
             -- (1) Canonical members of this series with no series override.
             -- A `series_index` override still drives ordering here even
             -- when the series itself is canonical, so a user who only
             -- repositions a book they didn't move keeps that order.
             SELECT bsl.book AS book_id,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series_index') IS NOT NULL
                        THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
                      ELSE b.series_index
                    END AS series_index
               FROM books_series_link bsl
               JOIN books b ON b.id = bsl.book
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE bsl.series = ?1
                AND (mo.book_uuid IS NULL
                     OR json_type(mo.overrides, '$.series') IS NULL)
             UNION
             -- (2) Books whose `overrides.series` names this series.
             SELECT b.id AS book_id,
                    -- NULLIF: an override that explicitly clears the index
                    -- (`Some("")` from the edit form) would otherwise CAST
                    -- to 0.0 and sort to the front. Treat empty-string as
                    -- "no index" and let NULLS LAST trail it.
                    CASE
                      WHEN json_type(mo.overrides, '$.series_index') IS NOT NULL
                        THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
                      ELSE b.series_index
                    END AS series_index
               FROM books b
               JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE json_type(mo.overrides, '$.series') IS NOT NULL
                AND json_extract(mo.overrides, '$.series') = ?2 COLLATE NOCASE
           )"#;

/// Run the `effective`-CTE + `BOOK_COLUMNS` SELECT and hydrate each row
/// into [`EbookMetadata`]. The nested vec is capped at
/// [`MAX_DISCOVERY_BOOKS`].
async fn fetch_series_books(
    pool: &SqlitePool,
    series_id: i64,
    series_name: &str,
) -> Result<Vec<EbookMetadata>, DiscoveryError> {
    let sql = format!(
        r#"{EFFECTIVE_SERIES_CTE}
           SELECT {BOOK_COLUMNS}
           FROM books b
           JOIN effective e ON e.book_id = b.id
           ORDER BY e.series_index NULLS LAST, b.sort, b.id
           LIMIT ?3"#
    );
    let rows = sqlx::query(&sql)
        .bind(series_id)
        .bind(series_name)
        .bind(MAX_DISCOVERY_BOOKS)
        .fetch_all(pool)
        .await?;

    let mut books = Vec::with_capacity(rows.len());
    for r in &rows {
        books.push(row_to_ebook(r)?);
    }
    Ok(books)
}

/// Count the (uncapped) effective membership for `(series_id, series_name)`
/// using the same UNION as [`fetch_series_books`] so truncation is
/// detectable as `book_count > books.len()`.
async fn count_effective_series_members(
    pool: &SqlitePool,
    series_id: i64,
    series_name: &str,
) -> Result<i64, sqlx::Error> {
    let sql = format!(
        r#"{EFFECTIVE_SERIES_CTE}
           SELECT COUNT(*) FROM effective"#
    );
    sqlx::query_scalar(&sql)
        .bind(series_id)
        .bind(series_name)
        .fetch_one(pool)
        .await
}

/// Bulk-merge overrides into each book and **unconditionally** pin
/// `series_id` / `series` to the parent series. A book that was canonically
/// in series A but overridden into series B would otherwise come back with
/// `series_id = Some(A)` (from the BOOK_COLUMNS subquery, which only reads
/// `books_series_link`) — so the card on B's page would link back to
/// /series/A. We're already on B's page by construction (the WHERE filter
/// matched the effective series name), so pinning to the requested series
/// is correct.
async fn merge_and_pin_series(
    pool: &SqlitePool,
    books: &mut [EbookMetadata],
    series_id: i64,
    series_name: &str,
) -> Result<(), sqlx::Error> {
    let uuids: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for book in books.iter_mut() {
        let uuid_owned = book.unique_identifier.clone();
        if let Some(uuid) = uuid_owned.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, uuid, ov, *has_cover_ov);
            }
        }
        book.series_id = Some(series_id);
        book.series = Some(series_name.to_string());
    }
    Ok(())
}
