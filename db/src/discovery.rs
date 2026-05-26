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
    //
    // Issue #154: the per-row `COUNT(*)` correlated subquery (O(tags ×
    // books)) is replaced with a single-pass `effective` membership CTE —
    // the UNION of (1) canonical `books_tags_link` rows whose book has no
    // `subjects` override and (2) override-extracted `(tag_name, book_id)`
    // pairs from `json_each(mo.overrides, '$.subjects')`. The per-tag count
    // is then a single scan of that union. The empty-array clear-all case
    // falls out naturally: a `Some([])` override drops the book from arm
    // (1) and yields no rows from `json_each` in arm (2). The override
    // match stays BINARY (`je.value = t.name`, no COLLATE) to match the
    // prior behavior. Visibility still requires ≥1 canonical link (the
    // `EXISTS`), so a tag that exists only inside override JSON never
    // surfaces.
    let rows = sqlx::query(
        r#"WITH effective AS (
             -- (1) Canonical tag memberships with no subjects override.
             SELECT btl.tag AS tag_id, NULL AS tag_name, btl.book AS book_id
               FROM books_tags_link btl
               JOIN books b ON b.id = btl.book
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE mo.book_uuid IS NULL
                 OR json_type(mo.overrides, '$.subjects') IS NULL
             UNION
             -- (2) Override-extracted subject memberships. UNION (not ALL)
             -- dedupes duplicate subject strings within one override array
             -- so a book with `["fiction","fiction"]` still counts once,
             -- matching the prior `EXISTS` semantics.
             SELECT NULL AS tag_id, je.value AS tag_name, b.id AS book_id
               FROM books b
               JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
               JOIN json_each(mo.overrides, '$.subjects') je
              WHERE json_type(mo.overrides, '$.subjects') IS NOT NULL
           )
           SELECT t.name,
             (SELECT COUNT(*) FROM effective e
               WHERE e.tag_id = t.id OR e.tag_name = t.name) AS cnt
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

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Discovery fixture seeders shared with `browse`, `books`, and
    //! `author_photos_data` tests. `pub(crate)` so siblings can call e.g.
    //! `use crate::discovery::test_helpers::seed_discovery_fixture;`.

    use crate::covers::test_helpers::CoversTempDir;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::sync::test_helpers::indexed;
    use sqlx::SqlitePool;

    // -------------------------------------------------------------------------
    // Discovery query tests (F1.8)
    // -------------------------------------------------------------------------

    /// Seed a small multi-author, multi-series, multi-tag fixture for the
    /// discovery query tests below. Returns the pool and a `CoversTempDir`
    /// guard the caller must keep alive for the lifetime of the test.
    pub(crate) async fn seed_discovery_fixture() -> (SqlitePool, CoversTempDir) {
        let guard = CoversTempDir::new("discovery");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                // Two-author book in Saga #1 with tag "fiction"
                indexed(
                    "saga1.epub",
                    Some("Saga: Book One"),
                    &["Ada Lovelace", "Grace Hopper"],
                    &["fiction", "classic"],
                    Some(("Saga", "1")),
                    None,
                ),
                // Sequel in Saga #2, same primary author + new tag
                indexed(
                    "saga2.epub",
                    Some("Saga: Book Two"),
                    &["Ada Lovelace"],
                    &["fiction"],
                    Some(("Saga", "2")),
                    None,
                ),
                // Standalone by Ada — no series
                indexed(
                    "standalone.epub",
                    Some("Standalone"),
                    &["Ada Lovelace"],
                    &["essay"],
                    None,
                    None,
                ),
                // Different-author, different-series book
                indexed(
                    "other.epub",
                    Some("Other Story"),
                    &["Niklaus Wirth"],
                    &["nonfiction"],
                    Some(("Pioneers", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        (pool, guard)
    }
    pub(crate) async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }
    pub(crate) async fn series_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }
    // -------------------------------------------------------------------------
    // Discovery read caps (issue #150)
    //
    // `get_author` / `get_series` previously serialized every attributed book
    // in one payload. The fix is a hard `LIMIT MAX_DISCOVERY_BOOKS` on the
    // nested `books` vec plus an uncapped `book_count` so callers can detect
    // truncation as `book_count > books.len()`.
    // -------------------------------------------------------------------------

    /// Seed `count` minimal `books` rows under `/lib`, all linked to one
    /// author ("Prolific") and one series ("Mega"), via recursive CTEs.
    /// Bypasses `replace_books`/the indexer — the cap only depends on link
    /// rows existing — keeping the test fast even past the 1k cap. Returns
    /// `(author_id, series_id)`.
    pub(crate) async fn seed_books_for_one_author_and_series(
        pool: &SqlitePool,
        count: i64,
    ) -> (i64, i64) {
        sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
            .execute(pool)
            .await
            .unwrap();
        let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
            .fetch_one(pool)
            .await
            .unwrap();
        let author_id: i64 = sqlx::query_scalar(
            "INSERT INTO authors (name, sort) VALUES ('Prolific', 'Prolific') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (name, sort) VALUES ('Mega', 'Mega') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort, series_index)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i), i
              FROM n
            "#,
        )
        .bind(count)
        .bind(lib_id)
        .execute(pool)
        .await
        .unwrap();
        // Link every seeded book to the author and the series.
        sqlx::query(
            "INSERT INTO books_authors_link (book, author, position)
             SELECT id, ?, 0 FROM books",
        )
        .bind(author_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO books_series_link (book, series)
             SELECT id, ? FROM books",
        )
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
        (author_id, series_id)
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;
    use crate::author_photos_data::{upsert_author_photo, AuthorPhotoSource};
    use crate::books::list_books;
    use crate::covers::test_helpers::CoversTempDir;
    use crate::metadata_overrides::upsert_metadata_overrides;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::sync::test_helpers::indexed;
    use omnibus_shared::{Contributor, MetadataOverrides};

    #[tokio::test]
    async fn get_author_returns_author_with_all_books_ordered_by_series_index() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = author_id_by_name(&pool, "Ada Lovelace").await;

        let author = get_author(&pool, id).await.unwrap().expect("author exists");

        assert_eq!(author.name, "Ada Lovelace");
        assert_eq!(author.book_count, 3);
        assert_eq!(author.books.len(), 3);

        // Series books come first, ordered by series_index ASC (NULLS LAST
        // means the standalone trails).
        let titles: Vec<_> = author
            .books
            .iter()
            .filter_map(|b| b.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ]
        );
    }
    #[tokio::test]
    async fn get_author_populates_series_id_on_books() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = author_id_by_name(&pool, "Ada Lovelace").await;
        let expected_sid = series_id_by_name(&pool, "Saga").await;

        let author = get_author(&pool, id).await.unwrap().unwrap();
        for book in author.books.iter().filter(|b| b.series.is_some()) {
            assert_eq!(
                book.series_id,
                Some(expected_sid),
                "series book should carry series_id"
            );
        }
        let standalone = author
            .books
            .iter()
            .find(|b| b.series.is_none())
            .expect("standalone present");
        assert_eq!(standalone.series_id, None);
    }
    #[tokio::test]
    async fn get_author_returns_none_for_missing_id() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let missing = get_author(&pool, 999_999).await.unwrap();
        assert!(missing.is_none());
    }
    #[tokio::test]
    async fn get_series_returns_books_ordered_by_series_index() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = series_id_by_name(&pool, "Saga").await;

        let series = get_series(&pool, id).await.unwrap().expect("series exists");
        assert_eq!(series.name, "Saga");
        assert_eq!(series.book_count, 2);

        let titles: Vec<_> = series
            .books
            .iter()
            .filter_map(|b| b.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book One".to_string(), "Saga: Book Two".to_string()]
        );
        // Each book should carry the parent series id back out so the
        // frontend can navigate cross-references without an extra lookup.
        for book in &series.books {
            assert_eq!(book.series_id, Some(id));
        }
    }
    #[tokio::test]
    async fn get_series_returns_none_for_missing_id() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let missing = get_series(&pool, 999_999).await.unwrap();
        assert!(missing.is_none());
    }
    #[tokio::test]
    async fn get_author_caps_books_at_max_discovery_books() {
        let _covers = CoversTempDir::new("author_cap");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_DISCOVERY_BOOKS + 25;
        let (author_id, _series_id) = seed_books_for_one_author_and_series(&pool, total).await;

        let author = get_author(&pool, author_id)
            .await
            .unwrap()
            .expect("author exists");
        assert_eq!(
            author.books.len() as i64,
            MAX_DISCOVERY_BOOKS,
            "get_author must cap the nested books vec at MAX_DISCOVERY_BOOKS"
        );
        assert_eq!(
            author.book_count as i64, total,
            "book_count must report the true (uncapped) shelf size"
        );
        assert!(
            author.book_count > author.books.len(),
            "truncation must be detectable as book_count > books.len()"
        );
    }
    #[tokio::test]
    async fn get_series_caps_books_at_max_discovery_books() {
        let _covers = CoversTempDir::new("series_cap");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_DISCOVERY_BOOKS + 25;
        let (_author_id, series_id) = seed_books_for_one_author_and_series(&pool, total).await;

        let series = get_series(&pool, series_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(
            series.books.len() as i64,
            MAX_DISCOVERY_BOOKS,
            "get_series must cap the nested books vec at MAX_DISCOVERY_BOOKS"
        );
        assert_eq!(
            series.book_count as i64, total,
            "book_count must report the true (uncapped) series size"
        );
        assert!(
            series.book_count > series.books.len(),
            "truncation must be detectable as book_count > books.len()"
        );
    }
    #[tokio::test]
    async fn get_tag_cloud_returns_counts_ordered_by_count_then_name() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let tags = get_tag_cloud(&pool).await.unwrap();

        // Fixture has: fiction × 2, classic × 1, essay × 1, nonfiction × 1.
        // Order: cnt DESC, then name ASC.
        let names: Vec<_> = tags.iter().map(|t| t.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "fiction".to_string(),
                "classic".to_string(),
                "essay".to_string(),
                "nonfiction".to_string(),
            ]
        );
        assert_eq!(tags[0].count, 2);
        assert!(tags[1..].iter().all(|t| t.count == 1));
    }
    #[tokio::test]
    async fn get_tag_cloud_returns_empty_vec_when_no_tags() {
        let _guard = CoversTempDir::new("empty_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        // No books, no tags.
        let tags = get_tag_cloud(&pool).await.unwrap();
        assert!(tags.is_empty());
    }
    #[tokio::test]
    async fn get_tag_cloud_counts_reflect_overrides() {
        // F5.1: per-tag counts in the cloud must follow the merged view.
        // Without the override-aware count, the cloud kept showing the
        // canonical totals — over-reporting tags the user had removed
        // from books and missing books whose tags were reassigned via
        // override.
        let _guard = CoversTempDir::new("tag_cloud_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["fiction"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["essay"], None, None),
            ],
        )
        .await
        .unwrap();

        // Sanity: canonical counts before any overrides.
        let pre = get_tag_cloud(&pool).await.unwrap();
        let fiction_pre = pre
            .iter()
            .find(|t| t.name == "fiction")
            .expect("fiction present pre-override");
        assert_eq!(fiction_pre.count, 2);

        // Reassign a.epub: drop "fiction", add "essay".
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["essay".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let post = get_tag_cloud(&pool).await.unwrap();
        let fiction = post
            .iter()
            .find(|t| t.name == "fiction")
            .expect("fiction still visible (canonical anchor remains on b.epub)");
        assert_eq!(
            fiction.count, 1,
            "fiction should drop a.epub after override, got {post:?}",
        );
        let essay = post
            .iter()
            .find(|t| t.name == "essay")
            .expect("essay present");
        assert_eq!(
            essay.count, 2,
            "essay should pick up override-tagged a.epub, got {post:?}",
        );
    }
    #[tokio::test]
    async fn get_author_includes_books_whose_override_names_this_author() {
        // Repro of the bug where renaming a book's author via the
        // metadata form (e.g. "Sanderson, Brandon" → "Brandon Sanderson")
        // left the book invisible on the new author's `/author/:id` page.
        // The override path writes JSON only — `books_authors_link` keeps
        // pointing at the canonical author row — so `get_author` must
        // layer overrides on top of the relational link at read time.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Set up the "Brandon Sanderson" vs "Sanderson, Brandon" shape:
        // one canonical author and a second name the user prefers, then
        // override one book to use the preferred name.
        let canonical_id = author_id_by_name(&pool, "Ada Lovelace").await;
        let preferred_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id",
        )
        .bind("Lovelace, Ada")
        .bind("Lovelace, Ada")
        .fetch_one(&pool)
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let saga_one_id = saga_one.id;

        // saga1.epub canonically lists ["Ada Lovelace", "Grace Hopper"];
        // the override renames the primary author to "Lovelace, Ada".
        let ov = MetadataOverrides {
            creators: Some(vec![
                Contributor {
                    name: "Lovelace, Ada".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                },
                Contributor {
                    name: "Grace Hopper".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                },
            ]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Visiting the preferred-name author page must now include the
        // overridden book, even though `books_authors_link` for that book
        // still points at the canonical "Ada Lovelace" row.
        let preferred = get_author(&pool, preferred_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = preferred
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book One".to_string()],
            "override-named author must surface the book on /author/:id",
        );

        // And the canonical-name author page must drop it, because the
        // override replaced the creator list wholesale.
        let canonical = get_author(&pool, canonical_id)
            .await
            .unwrap()
            .expect("author exists");
        let canonical_titles: Vec<_> = canonical
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            !canonical_titles.contains(&"Saga: Book One".to_string()),
            "override moved the book off the canonical author, got {canonical_titles:?}",
        );

        // The card on the preferred-name page should show the override
        // creator name, not the canonical one.
        let card = &preferred.books[0];
        assert_eq!(card.id, saga_one_id);
        assert_eq!(
            card.creators.first().map(|c| c.name.as_str()),
            Some("Lovelace, Ada")
        );
    }
    #[tokio::test]
    async fn get_author_excludes_books_whose_override_clears_authors() {
        // A book whose override sets creators to the empty array should
        // disappear from every author's page, matching what the book
        // detail page already shows (no breadcrumb author).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let ada = get_author(&pool, ada_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = ada
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            !titles.contains(&"Standalone".to_string()),
            "override-cleared creators must drop the book from /author/:id, got {titles:?}",
        );
    }
    #[tokio::test]
    async fn get_author_override_creator_match_is_case_insensitive() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`, so an override that
        // differs only by case from the target author's row must still
        // surface the book on `/author/:id`. The override comparison
        // gets an explicit `COLLATE NOCASE` because the LHS is a
        // `json_extract(...)` expression (BINARY by default) and the RHS
        // is a bound parameter (also no collation).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Override uses lowercase casing; canonical row is "Ada Lovelace".
        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ada lovelace".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let ada = get_author(&pool, ada_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = ada
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            titles.contains(&"Standalone".to_string()),
            "lowercase override should still match NOCASE author row, got {titles:?}",
        );
    }
    #[tokio::test]
    async fn get_series_includes_books_added_via_override() {
        // Repro of the bug where editing a book to set its series via the
        // metadata form left the book invisible on `/series/:id`. The
        // override path only writes JSON into `metadata_overrides` and
        // never touches `books_series_link`, so `get_series` must layer
        // overrides on top of the relational link at read time.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        // Loner has no canonical series at all. After the override it
        // should show up as #3 in Saga, after the two indexed books.
        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let standalone_uuid = standalone.unique_identifier.clone().unwrap();
        let standalone_id = standalone.id;

        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &standalone_uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(series.book_count, 3);

        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ],
            "override-set series_index=3 should sort the overridden book last",
        );

        // The overridden book must carry the parent series id so the card
        // links back to /series/:id.
        let overridden = series.books.iter().find(|b| b.id == standalone_id).unwrap();
        assert_eq!(overridden.series_id, Some(saga_id));
        assert_eq!(overridden.series.as_deref(), Some("Saga"));
    }
    #[tokio::test]
    async fn get_series_reorders_canonical_member_via_index_only_override() {
        // Issue #154 guard: a `series_index` override on a book that is
        // *already canonically* in this series (no `series` override) must
        // still drive ordering. The pre-#154 `effective` CTE computed the
        // index independently of the name; the single-pass UNION rewrite
        // must preserve that — otherwise repositioning a book you didn't
        // move silently no-ops on the series page.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        // "Saga: Book One" is canonically index 1, "Book Two" index 2.
        // Override Book One's index to 5 (no series change) so it now
        // trails Book Two.
        let books = list_books(&pool, "/lib").await.unwrap();
        let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = book_one.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            series_index: Some("5".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(
            series.book_count, 2,
            "membership is unchanged by an index-only override"
        );
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book Two".to_string(), "Saga: Book One".to_string()],
            "index-only override on a canonical member must re-sort it",
        );
    }
    #[tokio::test]
    async fn get_series_excludes_books_whose_override_clears_series() {
        // A book canonically in Saga whose override clears the series (sets
        // series to an empty string) should disappear from /series/:id,
        // matching what the book detail page already shows.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let book_two = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
        let uuid = book_two.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            series: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(series.book_count, 1);
        assert_eq!(
            series.books[0].title.as_deref(),
            Some("Saga: Book One"),
            "the unaffected book stays; the cleared one drops out",
        );
    }
    #[tokio::test]
    async fn get_series_override_match_is_case_insensitive() {
        // The CTE's `series_name` column is BINARY by default — without
        // `COLLATE NOCASE` on the filter, an override that differs only
        // by case from the canonical series row fails to match, even
        // though `series.name` is `UNIQUE COLLATE NOCASE`.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Override uses lowercase casing; canonical row is "Saga".
        let ov = MetadataOverrides {
            series: Some("saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            titles.contains(&"Standalone".to_string()),
            "lowercase override should still match NOCASE series row, got {titles:?}",
        );
    }
    #[tokio::test]
    async fn get_author_empty_string_series_index_sorts_last() {
        // Mirror of get_series: clearing the position field (`Some("")`)
        // used to CAST('') to 0.0 in get_author's ORDER BY and sort the
        // book to the front of the author's shelf. NULLIF drops it to NULL
        // so NULLS LAST trails it behind positioned books.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let author_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = book_one.unique_identifier.clone().unwrap();

        // Keep Book One (canonical Saga #1) but clear its position.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let author = get_author(&pool, author_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = author
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        let pos = |t: &str| titles.iter().position(|x| x == t).unwrap();
        assert!(
            pos("Saga: Book Two") < pos("Saga: Book One"),
            "cleared series_index should trail the positioned book, got {titles:?}",
        );
        assert_ne!(
            titles.first().map(String::as_str),
            Some("Saga: Book One"),
            "cleared series_index must not sort to the front, got {titles:?}",
        );
    }
    #[tokio::test]
    async fn get_series_empty_string_series_index_sorts_last() {
        // `Some("")` from the edit form (user cleared the position
        // field) was sorting to the front because `CAST('' AS REAL)`
        // returns 0.0. NULLIF on the override value drops it to NULL,
        // and ORDER BY ... NULLS LAST trails it after positioned books.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Add Standalone to Saga but clear its position.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ],
            "empty-string series_index should trail positioned books, not lead them",
        );
    }
    #[tokio::test]
    async fn get_series_pins_series_id_for_books_moved_between_series() {
        // A book canonically in Series A overridden into Series B used
        // to come back from get_series(B) with `series_id = Some(A)`
        // (BOOK_COLUMNS reads only books_series_link), so the card on
        // B's page would link back to /series/A. The fix pins
        // series_id/series unconditionally to the requested parent.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let pioneers_id = series_id_by_name(&pool, "Pioneers").await;

        // "Other Story" is canonically in Pioneers; override moves it
        // into Saga. Verify that opening Saga's page returns the book
        // pinned to Saga's id, not Pioneers'.
        let books = list_books(&pool, "/lib").await.unwrap();
        let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
        let uuid = other.unique_identifier.clone().unwrap();

        let saga_id = series_id_by_name(&pool, "Saga").await;
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("5".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let saga = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("Saga exists");
        let moved = saga
            .books
            .iter()
            .find(|b| b.title.as_deref() == Some("Other Story"))
            .expect("override moved Other Story into Saga");
        assert_eq!(
            moved.series_id,
            Some(saga_id),
            "card on Saga's page must link back to Saga, not the canonical Pioneers",
        );
        assert_eq!(moved.series.as_deref(), Some("Saga"));

        // And it should be gone from Pioneers' page.
        let pioneers = get_series(&pool, pioneers_id)
            .await
            .unwrap()
            .expect("Pioneers exists");
        assert!(
            !pioneers
                .books
                .iter()
                .any(|b| b.title.as_deref() == Some("Other Story")),
            "override moved Other Story off Pioneers",
        );
    }
    #[tokio::test]
    async fn get_author_populates_has_photo() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // No row → false.
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(!ada.has_photo, "no row should yield has_photo = false");

        // Letter marker → still false (negative-cache shouldn't render an img).
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(
            !ada.has_photo,
            "letter marker should yield has_photo = false"
        );

        // Manual upload → true.
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfake"),
        )
        .await
        .unwrap();
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(ada.has_photo, "manual upload should yield has_photo = true");
    }
}
