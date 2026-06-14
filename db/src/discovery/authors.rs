//! Author-detail read: a single author plus their books across every
//! library. Membership and ordering layer overrides on top of the
//! canonical `books_authors_link` so the form-edited creator list drives
//! both the shelf contents and the per-card creator names.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{AuthorDetail, EbookMetadata};

use crate::books::{backfill_creator_ids, row_to_ebook, BOOK_COLUMNS};
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk};

use super::DiscoveryError;

/// Hard cap on the nested `books` vec returned by the discovery-detail
/// reads ([`get_author`] / [`super::get_series`]). Truncation is surfaced
/// via `book_count` (uncapped `COUNT(*)`) so callers detect it as
/// `book_count > books.len()`.
pub const MAX_DISCOVERY_BOOKS: i64 = 1_000;

/// Fetch an author by ID with their books across every library. Returns
/// `None` if the author ID doesn't exist. The nested `books` vec is
/// capped at [`MAX_DISCOVERY_BOOKS`]; `book_count` is uncapped.
pub async fn get_author(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<AuthorDetail>, DiscoveryError> {
    // TODO: scope by `user_id` once per-user ACLs land (single-tenant today).
    let author_row = sqlx::query("SELECT id, name, sort FROM authors WHERE id = ?")
        .bind(author_id)
        .fetch_optional(pool)
        .await?;
    let Some(a) = author_row else {
        return Ok(None);
    };
    let author_name: String = a.get("name");

    let mut books = fetch_author_books(pool, author_id, &author_name).await?;
    let book_count = count_effective_author_members(pool, author_id, &author_name).await?;
    merge_overrides_into_author_books(pool, &mut books).await?;
    backfill_creator_ids(pool, &mut books).await?;
    let has_photo = author_has_usable_photo(pool, author_id).await?;

    Ok(Some(AuthorDetail {
        id: a.get("id"),
        name: a.get("name"),
        sort: a.get("sort"),
        book_count: book_count as usize,
        books,
        has_photo,
    }))
}

/// SQL CTE expressing the override-aware effective author membership. Used
/// by both `fetch_author_books` (with the full `BOOK_COLUMNS` SELECT) and
/// `count_effective_author_members` (with `COUNT(*)`).
///
/// `apply_overrides` replaces creators wholesale when the override JSON
/// has the `creators` key, so the effective creator set for a book is:
/// - `overrides.creators` if the JSON has that key (including the empty
///   array, which clears all authors), else
/// - the canonical names from `books_authors_link`.
///
/// Single-pass UNION over (1) canonical link rows for *this* author whose
/// book has no `creators` override, and (2) override rows whose
/// `overrides.creators` array names this author. Arm (1) is scoped to
/// `bal.author = ?2` up front so it drives through the
/// `idx_books_authors_author` reverse index instead of scanning every
/// book — mirroring the `EFFECTIVE_SERIES_CTE` shape. Override creators
/// carry only `name`, so arm (2) matches by name (NOCASE) against `?1`.
///
/// Each arm carries the override-aware `series_index` (the same NULLIF
/// rule `get_series` uses) so the detail page can order by it. An
/// index-only override on a canonical member still re-sorts here because
/// arm (1) reads `mo.overrides.series_index` even when creators are
/// canonical. Bind order: `?1` = author_name, `?2` = author_id.
const EFFECTIVE_AUTHOR_CTE: &str = r#"WITH effective AS (
             -- (1) Canonical authorship of this author with no creators
             -- override. A `series_index` override still drives ordering.
             SELECT bal.book AS book_id,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series_index') IS NOT NULL
                        THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
                      ELSE b.series_index
                    END AS series_index
               FROM books_authors_link bal
               JOIN books b ON b.id = bal.book
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE bal.author = ?2
                AND (mo.book_uuid IS NULL
                     OR json_type(mo.overrides, '$.creators') IS NULL)
             UNION
             -- (2) Books whose `overrides.creators` names this author.
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
              WHERE json_type(mo.overrides, '$.creators') IS NOT NULL
                AND EXISTS (
                  SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                   WHERE json_extract(je.value, '$.name') = ?1 COLLATE NOCASE
                )
           )"#;

/// Run the `effective`-CTE + `BOOK_COLUMNS` SELECT and hydrate each row
/// into [`EbookMetadata`]. `series_index` ordering follows the same
/// override-aware NULLIF rule used in `get_series`. The returned vec is
/// capped at [`MAX_DISCOVERY_BOOKS`].
async fn fetch_author_books(
    pool: &SqlitePool,
    author_id: i64,
    author_name: &str,
) -> Result<Vec<EbookMetadata>, DiscoveryError> {
    let sql = format!(
        r#"{EFFECTIVE_AUTHOR_CTE}
           SELECT {BOOK_COLUMNS}
           FROM books b
           JOIN effective e ON e.book_id = b.id
           ORDER BY e.series_index NULLS LAST, b.sort, b.id
           LIMIT ?3"#
    );
    let rows = sqlx::query(&sql)
        .bind(author_name)
        .bind(author_id)
        .bind(MAX_DISCOVERY_BOOKS)
        .fetch_all(pool)
        .await?;

    let mut books = Vec::with_capacity(rows.len());
    for r in &rows {
        books.push(row_to_ebook(r)?);
    }
    Ok(books)
}

/// Count the (uncapped) effective shelf size using the same UNION as
/// [`fetch_author_books`] so `book_count` stays truthful and truncation is
/// detectable as `book_count > books.len()`.
async fn count_effective_author_members(
    pool: &SqlitePool,
    author_id: i64,
    author_name: &str,
) -> Result<i64, sqlx::Error> {
    let sql = format!(
        r#"{EFFECTIVE_AUTHOR_CTE}
           SELECT COUNT(*) FROM effective"#
    );
    sqlx::query_scalar(&sql)
        .bind(author_name)
        .bind(author_id)
        .fetch_one(pool)
        .await
}

/// Bulk-apply overrides into each book so card titles / descriptions /
/// covers reflect user edits, matching what `list_books` does for the
/// landing grid. Unlike `merge_and_pin_series`, the author detail page
/// does **not** rewrite `series_id` — only the merge happens here.
async fn merge_overrides_into_author_books(
    pool: &SqlitePool,
    books: &mut [EbookMetadata],
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
    }
    Ok(())
}

/// Whether a usable profile photo is cached for `author_id`. F1.11 — the
/// frontend uses this to render `<img>` vs the typographic letter avatar
/// in one round trip. `'letter'` rows are the negative-cache marker and
/// do not count as a usable photo.
async fn author_has_usable_photo(pool: &SqlitePool, author_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM author_photos
              WHERE author_id = ?
                AND source IN ('manual', 'openlibrary')
                AND bytes IS NOT NULL
         )",
    )
    .bind(author_id)
    .fetch_one(pool)
    .await
}
