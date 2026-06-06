//! Author-detail read: a single author plus their books across every
//! library. Membership and ordering layer overrides on top of the
//! canonical `books_authors_link` so the form-edited creator list drives
//! both the shelf contents and the per-card creator names.

use sqlx::{Row, SqlitePool};

use omnibus_shared::AuthorDetail;

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

    // F5.1: membership and ordering follow the merged (override-aware)
    // view, not the raw `books_authors_link` table. `apply_overrides`
    // replaces creators wholesale when the override JSON has the
    // `creators` key, so the effective creator set for a book is:
    //   - `overrides.creators` if the JSON has that key (including the
    //     empty array, which clears all authors), else
    //   - the canonical names from `books_authors_link`.
    // Override creators carry only `name`, so we match by name against
    // the target author. `series_index` overrides drive ordering the same
    // way as in `get_series`.
    let sql = format!(
        r#"SELECT {BOOK_COLUMNS}
           FROM books b
           LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE
             CASE
               WHEN mo.book_uuid IS NOT NULL
                    AND json_type(mo.overrides, '$.creators') IS NOT NULL
                 THEN EXISTS (
                   SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                    WHERE json_extract(je.value, '$.name') = ? COLLATE NOCASE
                 )
               ELSE EXISTS (
                 SELECT 1 FROM books_authors_link bal
                  WHERE bal.book = b.id AND bal.author = ?
               )
             END
           ORDER BY
             CASE
               WHEN mo.book_uuid IS NOT NULL
                    AND json_type(mo.overrides, '$.series_index') IS NOT NULL
                 -- NULLIF: an override that explicitly clears the index
                 -- (`Some("")` from the edit form) would otherwise CAST to
                 -- 0.0 and sort to the front. Treat empty-string as "no
                 -- index" so ORDER BY's NULLS LAST trails it — matching
                 -- get_series.
                 THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
               ELSE b.series_index
             END NULLS LAST,
             b.sort, b.id
           LIMIT ?"#
    );
    let rows = sqlx::query(&sql)
        .bind(&author_name)
        .bind(author_id)
        .bind(MAX_DISCOVERY_BOOKS)
        .fetch_all(pool)
        .await?;

    let mut books = Vec::with_capacity(rows.len());
    for r in &rows {
        books.push(row_to_ebook(r)?);
    }

    // Issue #150: `books` above is capped at `MAX_DISCOVERY_BOOKS`, so its
    // length can no longer stand in for the author's true shelf size.
    // Count the (uncapped) effective membership separately using the same
    // override-aware predicate as the SELECT so `book_count` stays
    // truthful and callers can detect truncation as
    // `book_count > books.len()`.
    let book_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM books b
           LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE
             CASE
               WHEN mo.book_uuid IS NOT NULL
                    AND json_type(mo.overrides, '$.creators') IS NOT NULL
                 THEN EXISTS (
                   SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                    WHERE json_extract(je.value, '$.name') = ? COLLATE NOCASE
                 )
               ELSE EXISTS (
                 SELECT 1 FROM books_authors_link bal
                  WHERE bal.book = b.id AND bal.author = ?
               )
             END"#,
    )
    .bind(&author_name)
    .bind(author_id)
    .fetch_one(pool)
    .await?;

    // Bulk-apply overrides so card titles / descriptions / covers reflect
    // user edits, matching what `list_books` does for the landing grid.
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
    }
    backfill_creator_ids(pool, &mut books).await?;

    // F1.11: surface whether a usable profile photo is cached so the
    // frontend can render <img> vs the typographic letter avatar in one
    // round trip. `'letter'` rows are the negative-cache marker and do
    // not count as a usable photo.
    let has_photo: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM author_photos
              WHERE author_id = ?
                AND source IN ('manual', 'openlibrary')
                AND bytes IS NOT NULL
         )",
    )
    .bind(author_id)
    .fetch_one(pool)
    .await?;

    Ok(Some(AuthorDetail {
        id: a.get("id"),
        name: a.get("name"),
        sort: a.get("sort"),
        book_count: book_count as usize,
        books,
        has_photo,
    }))
}
