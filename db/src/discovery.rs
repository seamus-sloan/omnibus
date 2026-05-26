//! F1.8 discovery-detail reads: a single author or series with their
//! books, plus the global tag cloud. Membership and ordering follow the
//! merged (override-aware) view via the BOOK_COLUMNS template shared
//! with the book read path.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{AuthorDetail, SeriesDetail, TagWeight};

use crate::books::{backfill_creator_ids, row_to_ebook, BOOK_COLUMNS};
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk};

/// Hard cap on the nested `books` vec returned by the discovery-detail
/// reads ([`get_author`] / [`get_series`]). Issue #150: these functions
/// previously serialized *every* book attributed to an author or series
/// in one payload — a prolific reference author or a giant Calibre series
/// could nest thousands of `EbookMetadata` structs (each with its own
/// `Vec<Contributor>` / `Vec` subjects / `Vec<Identifier>`) into a single
/// response. 1 000 is far above any realistic single-author/series shelf a
/// client renders in one grid, yet keeps the JSON envelope bounded.
///
/// Truncation is surfaced *without* a struct change: `AuthorDetail` /
/// `SeriesDetail` already carry a `book_count` field. Both reads now set
/// `book_count` from a dedicated uncapped `COUNT(*)`, so a caller detects
/// truncation as `book_count > books.len()`. The `X-Total-Count` header
/// floated in #150 doesn't fit here — these flow through Dioxus server
/// functions (`frontend/src/rpc.rs`), not raw axum handlers, so there's no
/// ergonomic place to set a response header. Cursor pagination is the
/// intended F4.x follow-up; see `docs/roadmap/`.
pub const MAX_DISCOVERY_BOOKS: i64 = 1_000;

/// Fetch an author by ID with their books across every library. Returns
/// `None` if the author ID doesn't exist.
///
/// # Bounded reads (issue #150)
///
/// The nested `books` vec is hard-capped at [`MAX_DISCOVERY_BOOKS`]; a
/// prolific reference author no longer serializes thousands of nested
/// `EbookMetadata` structs in one payload. `book_count` is computed from a
/// separate uncapped `COUNT(*)`, so it always reports the true shelf size
/// and callers detect truncation as `book_count > books.len()`. Cursor
/// pagination is the intended F4.x follow-up.
///
/// # Multi-tenancy
///
/// This function returns all matching rows without filtering by the
/// requesting user's library access. The app is single-tenant and
/// single-library today (see Phase 4 in `docs/roadmap/0-0-summary.md`),
/// so every authenticated caller is implicitly authorised to see every
/// author. When per-user ACLs land in the F4.x phase this function must
/// accept a `user_id` parameter and filter both the `authors` row and
/// the joined `books` via the access-control join. The `TODO(F4.x)`
/// marker in the body stays in place until that work lands.
pub async fn get_author(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<AuthorDetail>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land. See the
    // function-level rustdoc above for the single-tenant rationale.
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
        if let Some(uuid) = book.unique_identifier.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, ov, *has_cover_ov);
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

/// Fetch a series by ID with its books, ordered by series index. Returns
/// `None` if the series ID doesn't exist.
///
/// # Bounded reads (issue #150)
///
/// The nested `books` vec is hard-capped at [`MAX_DISCOVERY_BOOKS`]; a
/// giant Calibre series no longer serializes its entire shelf in one
/// payload. `book_count` is computed from a separate uncapped `COUNT(*)`,
/// so it reports the true series size and truncation is detectable as
/// `book_count > books.len()`. Cursor pagination is the intended F4.x
/// follow-up.
///
/// # Multi-tenancy
///
/// This function returns all matching rows without filtering by the
/// requesting user's library access. The app is single-tenant and
/// single-library today (see Phase 4 in `docs/roadmap/0-0-summary.md`),
/// so every authenticated caller is implicitly authorised to see every
/// series. When per-user ACLs land in the F4.x phase this function must
/// accept a `user_id` parameter and filter both the `series` row and the
/// joined `books` via the access-control join — same caveat as
/// [`get_author`]. The `TODO(F4.x)` marker in the body stays in place
/// until that work lands.
pub async fn get_series(
    pool: &SqlitePool,
    series_id: i64,
) -> Result<Option<SeriesDetail>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land. See the
    // function-level rustdoc above for the single-tenant rationale.
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
    let sql = format!(
        r#"WITH effective AS (
             SELECT b.id AS book_id,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series') IS NOT NULL
                        THEN json_extract(mo.overrides, '$.series')
                      ELSE (SELECT s2.name FROM books_series_link bsl
                              JOIN series s2 ON s2.id = bsl.series
                             WHERE bsl.book = b.id LIMIT 1)
                    END AS series_name,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series_index') IS NOT NULL
                        -- NULLIF: an override that explicitly clears the
                        -- index (`Some("")` from the edit form) would
                        -- otherwise CAST to 0.0 and sort to the front of
                        -- the series. Treat empty-string as "no index"
                        -- and let ORDER BY's NULLS LAST trail it.
                        THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
                      ELSE b.series_index
                    END AS series_index
               FROM books b
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           )
           SELECT {BOOK_COLUMNS}
           FROM books b
           JOIN effective e ON e.book_id = b.id
           WHERE e.series_name = ? COLLATE NOCASE
           ORDER BY e.series_index NULLS LAST, b.sort, b.id
           LIMIT ?"#
    );
    let rows = sqlx::query(&sql)
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
    // override-aware `effective` CTE — so `book_count` reflects the true
    // series size and truncation is detectable as
    // `book_count > books.len()`.
    let book_count: i64 = sqlx::query_scalar(
        r#"WITH effective AS (
             SELECT b.id AS book_id,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series') IS NOT NULL
                        THEN json_extract(mo.overrides, '$.series')
                      ELSE (SELECT s2.name FROM books_series_link bsl
                              JOIN series s2 ON s2.id = bsl.series
                             WHERE bsl.book = b.id LIMIT 1)
                    END AS series_name
               FROM books b
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           )
           SELECT COUNT(*)
           FROM effective e
           WHERE e.series_name = ? COLLATE NOCASE"#,
    )
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
        if let Some(uuid) = book.unique_identifier.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, ov, *has_cover_ov);
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

/// Maximum number of tags returned from [`get_tag_cloud`]. Caps the
/// payload so a Calibre dump with 10k+ unique subjects can't blow up
/// the client or stall the SQLite pool on serialization.
const TAG_CLOUD_LIMIT: i64 = 500;

/// Return up to [`TAG_CLOUD_LIMIT`] tags with their book counts, ordered
/// by count descending then name ascending. Used by the tag cloud page.
///
/// # Bounded reads (issue #150)
///
/// Unlike [`get_author`] / [`get_series`], this read never nests book
/// lists — it returns only `TagWeight { name, count }` rows — and the tag
/// list itself is already capped at [`TAG_CLOUD_LIMIT`]. There is nothing
/// unbounded to cap here; the #150 work is the discovery-detail reads.
///
/// # Multi-tenancy
///
/// This function returns the global tag distribution without filtering
/// by the requesting user's library access. The app is single-tenant and
/// single-library today (see Phase 4 in `docs/roadmap/0-0-summary.md`),
/// so every authenticated caller sees the same tag cloud. When per-user
/// ACLs land in the F4.x phase this function must accept a `user_id`
/// parameter and count only books visible to that user via the
/// access-control join. The `TODO(F4.x)` marker in the body stays in
/// place until that work lands.
pub async fn get_tag_cloud(pool: &SqlitePool) -> Result<Vec<TagWeight>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land. See the
    // function-level rustdoc above for the single-tenant rationale.
    //
    // F5.1: counts use the effective (override-aware) subject set, not
    // the raw `books_tags_link` rows — `overrides.subjects` replaces a
    // book's canonical tag list wholesale when Some. Visibility still
    // requires at least one canonical link so tags that exist only as a
    // string inside override JSON don't surface (no `tags` row to point
    // at). The single-library, single-tenant scope means the cloud is
    // global; if/when per-library scoping lands this picks up a path
    // filter alongside the existing `WHERE EXISTS`.
    let rows = sqlx::query(
        r#"SELECT t.name,
             (SELECT COUNT(*)
                FROM books b
                LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
               WHERE CASE
                       WHEN mo.book_uuid IS NOT NULL
                            AND json_type(mo.overrides, '$.subjects') IS NOT NULL
                         THEN EXISTS (
                           SELECT 1 FROM json_each(mo.overrides, '$.subjects') je
                            WHERE je.value = t.name
                         )
                       ELSE EXISTS (
                         SELECT 1 FROM books_tags_link btl
                          WHERE btl.book = b.id AND btl.tag = t.id
                       )
                     END
             ) AS cnt
           FROM tags t
           WHERE EXISTS (
             SELECT 1 FROM books_tags_link btl WHERE btl.tag = t.id
           )
           ORDER BY cnt DESC, t.name ASC
           LIMIT ?"#,
    )
    .bind(TAG_CLOUD_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TagWeight {
            name: r.get("name"),
            count: r.get::<i64, _>("cnt") as usize,
        })
        .collect())
}
