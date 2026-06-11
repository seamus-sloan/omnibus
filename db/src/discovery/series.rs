//! Series-detail read: a single series plus its books, ordered by the
//! override-aware series index. A single-pass `effective` CTE merges the
//! canonical `books_series_link` membership with the form-edited override
//! set so books moved into or out of a series via the edit form surface
//! here too.

use sqlx::{Row, SqlitePool};

use omnibus_shared::SeriesDetail;

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

    // F5.1: membership and ordering follow the merged (override-aware)
    // view, not the raw `books_series_link` table. `upsert_metadata_overrides`
    // never writes to the relational link tables, so a book added to a
    // series purely through the edit form would otherwise be invisible
    // here even though `apply_overrides` shows it in that series everywhere
    // else (book detail, landing grid). The effective series for a book is:
    //   - `overrides.series` if the JSON has that key (including the empty
    //     string, which means "clear the series"), else
    //   - the canonical name from `books_series_link`.
    // Same fallback for `series_index` so the override-set position drives
    // ordering when present.
    //
    // Issue #154: the membership set is built as a single-pass UNION over
    // (1) canonical link rows for *this* series whose book has no `series`
    // override, and (2) override rows whose `overrides.series` matches this
    // series' name. Both arms are scoped to the target series up front, so
    // arm (1) drives through the `books_series_link(series)` index instead
    // of scanning every book and computing a per-row correlated subquery.
    // The empty-string clear-all case is handled naturally: a `Some("")`
    // override removes the book from arm (1) (override IS NOT NULL) and the
    // `'' = name` filter excludes it from arm (2). Case-insensitivity is
    // preserved by the `COLLATE NOCASE` on arm (2)'s name comparison
    // (canonical arm (1) needs none — it joins on `series.id`).
    let sql = format!(
        r#"WITH effective AS (
             -- (1) Canonical members of this series with no series override.
             -- A `series_index` override still drives ordering here even
             -- when the series itself is canonical (the original `effective`
             -- CTE computed the index independently of the name), so a user
             -- who only repositions a book they didn't move keeps that order.
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
                    -- to 0.0 and sort to the front of the series. Treat
                    -- empty-string as "no index" and let NULLS LAST trail it.
                    CASE
                      WHEN json_type(mo.overrides, '$.series_index') IS NOT NULL
                        THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
                      ELSE b.series_index
                    END AS series_index
               FROM books b
               JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE json_type(mo.overrides, '$.series') IS NOT NULL
                AND json_extract(mo.overrides, '$.series') = ?2 COLLATE NOCASE
           )
           SELECT {BOOK_COLUMNS}
           FROM books b
           JOIN effective e ON e.book_id = b.id
           ORDER BY e.series_index NULLS LAST, b.sort, b.id
           LIMIT ?3"#
    );
    let rows = sqlx::query(&sql)
        .bind(series_id)
        .bind(&series_name)
        .bind(MAX_DISCOVERY_BOOKS)
        .fetch_all(pool)
        .await?;

    let mut books = Vec::with_capacity(rows.len());
    for r in &rows {
        books.push(row_to_ebook(r)?);
    }

    // Issue #150: `books` is capped at `MAX_DISCOVERY_BOOKS`. Count the
    // uncapped effective membership separately — reusing the same
    // single-pass effective-membership UNION as the SELECT (issue #154) —
    // so `book_count` reflects the true series size and truncation is
    // detectable as `book_count > books.len()`.
    let book_count: i64 = sqlx::query_scalar(
        r#"WITH effective AS (
             SELECT bsl.book AS book_id
               FROM books_series_link bsl
               JOIN books b ON b.id = bsl.book
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE bsl.series = ?1
                AND (mo.book_uuid IS NULL
                     OR json_type(mo.overrides, '$.series') IS NULL)
             UNION
             SELECT b.id AS book_id
               FROM books b
               JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE json_type(mo.overrides, '$.series') IS NOT NULL
                AND json_extract(mo.overrides, '$.series') = ?2 COLLATE NOCASE
           )
           SELECT COUNT(*) FROM effective"#,
    )
    .bind(series_id)
    .bind(&series_name)
    .fetch_one(pool)
    .await?;

    // Merge overrides into each book so the series-card title/description
    // /cover reflect user edits, matching what `list_books` does for the
    // landing grid.
    let uuids: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for book in &mut books {
        let uuid_owned = book.unique_identifier.clone();
        if let Some(uuid) = uuid_owned.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, uuid, ov, *has_cover_ov);
            }
        }
        // Pin `series_id` / `series` to the parent series for every
        // returned row, not just the override-only ones. A book that was
        // canonically in series A but overridden into series B would
        // otherwise come back here with `series_id = Some(A)` (from the
        // BOOK_COLUMNS subquery, which only reads `books_series_link`)
        // — so the card on B's page would link back to /series/A. We're
        // already on B's page by construction (the WHERE filter matched
        // the effective series name), so unconditionally pinning to the
        // requested series is correct.
        book.series_id = Some(series_id);
        book.series = Some(series_name.clone());
    }
    backfill_creator_ids(pool, &mut books).await?;

    Ok(Some(SeriesDetail {
        id: s.get("id"),
        name: s.get("name"),
        sort: s.get("sort"),
        book_count: book_count as usize,
        books,
    }))
}
