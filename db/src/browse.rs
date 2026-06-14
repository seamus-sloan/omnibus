//! Browse-all index pages: `/authors` and `/series`. Returns every row
//! (capped at `INDEX_LIMIT`) so the UI's client-side sort/filter has the
//! full list to work with; per-row counts come back override-aware so the
//! index stays consistent with the discovery-detail reads. Counts are
//! computed in a single reverse-index-driven `GROUP BY` pass (one
//! `effective` membership CTE, not a correlated subquery per row) so a
//! multi-thousand-author library doesn't pay an O(rows × books) cost.
//! Single-tenant today — no per-user ACL filtering.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{AuthorSummary, SeriesSummary};

/// Errors returned by the browse index queries.
#[derive(Debug, thiserror::Error)]
pub enum BrowseError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Hard cap on rows returned by [`list_authors`] / [`list_series`]. Keeps
/// the JSON envelope under ~1 MB even with the optional accent string,
/// while leaving headroom past a 5k+ author library.
const INDEX_LIMIT: i64 = 10_000;

/// Build a `VALUES (?)` list for a CTE that materializes the library-path
/// set once. At most two entries (ebook + audiobook), so the bind count
/// stays trivial.
fn placeholders(n: usize) -> String {
    std::iter::repeat_n("(?)", n).collect::<Vec<_>>().join(", ")
}

/// Return every author with their book count and an optional cover-derived
/// accent, scoped to `library_paths`, ordered by name ascending and capped
/// at [`INDEX_LIMIT`]. Empty list when no paths match or the slice is empty.
///
/// Currently returns results across all users (single-tenant). When F4.x
/// per-user ACL lands, add a `user_id: i64` parameter and scope the query
/// to books accessible to that user.
pub async fn list_authors(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<Vec<AuthorSummary>, BrowseError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let ph = placeholders(library_paths.len());
    // `effective(author_id, book_id)` is the override-aware membership set,
    // built once: arm (1) canonical link rows whose book has no creators
    // override, arm (2) override-derived `(authors.id, book_id)` pairs.
    // A single `GROUP BY author_id` over it replaces the old per-author
    // correlated `COUNT(*)` subquery, so the books table is scanned once
    // rather than once per author. UNION (not ALL) collapses a duplicate
    // author name inside one override array, matching the old `EXISTS`.
    let sql = format!(
        r#"
        WITH lib_paths(p) AS (VALUES {ph}),
        effective AS (
            -- (1) Canonical authorship with no creators override.
            SELECT bal.author AS author_id, bal.book AS book_id
              FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN scan_roots l ON l.id = b.library_id
              LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
             WHERE l.path IN (SELECT p FROM lib_paths)
               AND (mo.book_uuid IS NULL
                    OR json_type(mo.overrides, '$.creators') IS NULL)
            UNION
            -- (2) Override creators resolved (NOCASE) to an authors row.
            SELECT a2.id AS author_id, b.id AS book_id
              FROM books b
              JOIN scan_roots l ON l.id = b.library_id
              JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              JOIN json_each(mo.overrides, '$.creators') je
              JOIN authors a2
                ON a2.name = json_extract(je.value, '$.name') COLLATE NOCASE
             WHERE l.path IN (SELECT p FROM lib_paths)
               AND json_type(mo.overrides, '$.creators') IS NOT NULL
        ),
        counts AS (
            SELECT author_id, COUNT(*) AS book_count
              FROM effective
             GROUP BY author_id
        )
        SELECT a.id, a.name, a.sort,
               COALESCE(c.book_count, 0) AS book_count,
               (SELECT b2.accent_color
                  FROM books_authors_link bal2
                  JOIN books b2 ON b2.id = bal2.book
                  JOIN scan_roots l2 ON l2.id = b2.library_id
                 WHERE bal2.author = a.id
                   AND l2.path IN (SELECT p FROM lib_paths)
                   AND b2.accent_color IS NOT NULL
                 ORDER BY b2.sort, b2.id
                 LIMIT 1) AS accent,
               EXISTS(
                 SELECT 1 FROM author_photos ap
                  WHERE ap.author_id = a.id
                    AND ap.source IN ('manual', 'openlibrary')
                    AND ap.bytes IS NOT NULL
               ) AS has_photo
        FROM authors a
        LEFT JOIN counts c ON c.author_id = a.id
        WHERE EXISTS (
            SELECT 1 FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE bal.author = a.id
               AND l.path IN (SELECT p FROM lib_paths)
          )
        ORDER BY COALESCE(a.sort, a.name) COLLATE NOCASE ASC
        LIMIT ?
        "#
    );
    let mut q = sqlx::query(&sql);
    for path in library_paths {
        q = q.bind(*path);
    }
    q = q.bind(INDEX_LIMIT);
    let rows = q.fetch_all(pool).await?;

    Ok(rows
        .iter()
        .map(|r| AuthorSummary {
            id: r.get("id"),
            name: r.get("name"),
            sort: r.get("sort"),
            book_count: usize::try_from(r.get::<i64, _>("book_count")).unwrap_or(0),
            accent: r.get("accent"),
            has_photo: r.get::<i64, _>("has_photo") != 0,
        })
        .collect())
}

/// Return every series with book count, primary author, and an optional
/// accent, scoped to `library_paths`, ordered by name ascending and capped
/// at [`INDEX_LIMIT`]. Empty list when no paths match or the slice is empty.
///
/// Currently returns results across all users (single-tenant). When F4.x
/// per-user ACL lands, add a `user_id: i64` parameter and scope the query
/// to books accessible to that user.
pub async fn list_series(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<Vec<SeriesSummary>, BrowseError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let sql = series_index_sql(library_paths.len());
    let mut q = sqlx::query(&sql);
    for path in library_paths {
        q = q.bind(*path);
    }
    q = q.bind(INDEX_LIMIT);
    let rows = q.fetch_all(pool).await?;
    Ok(rows.iter().map(map_series_row).collect())
}

/// Build the `list_series` SQL for `n` library-path placeholders. Uses a CTE
/// to materialize the path set once so the correlated subqueries (accent,
/// primary author) each see the same `lib_paths` without repeated inline
/// VALUES lists. `book_count` comes from a single `GROUP BY` over the
/// `effective` membership set rather than a per-series correlated subquery,
/// so the books table is scanned once instead of once per series.
fn series_index_sql(n: usize) -> String {
    let ph = placeholders(n);
    format!(
        r#"
        WITH lib_paths(p) AS (VALUES {ph}),
        effective AS (
            -- (1) Canonical members with no series override.
            SELECT bsl.series AS series_id, bsl.book AS book_id
              FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN scan_roots l ON l.id = b.library_id
              LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
             WHERE l.path IN (SELECT p FROM lib_paths)
               AND (mo.book_uuid IS NULL
                    OR json_type(mo.overrides, '$.series') IS NULL)
            UNION
            -- (2) Books whose `overrides.series` names a series (NOCASE).
            SELECT s2.id AS series_id, b.id AS book_id
              FROM books b
              JOIN scan_roots l ON l.id = b.library_id
              JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              JOIN series s2
                ON s2.name = json_extract(mo.overrides, '$.series') COLLATE NOCASE
             WHERE l.path IN (SELECT p FROM lib_paths)
               AND json_type(mo.overrides, '$.series') IS NOT NULL
        ),
        counts AS (
            SELECT series_id, COUNT(*) AS book_count
              FROM effective
             GROUP BY series_id
        )
        SELECT s.id, s.name, s.sort,
               COALESCE(c.book_count, 0) AS book_count,
               (SELECT
                  CASE
                    WHEN mo2.book_uuid IS NOT NULL
                         AND json_type(mo2.overrides, '$.creators') IS NOT NULL
                      THEN json_extract(mo2.overrides, '$.creators[0].name')
                    ELSE (SELECT a.name FROM books_authors_link bal
                            JOIN authors a ON a.id = bal.author
                           WHERE bal.book = b2.id
                           ORDER BY bal.position LIMIT 1)
                  END
                FROM books_series_link bsl2
                  JOIN books b2 ON b2.id = bsl2.book
                  JOIN scan_roots l2 ON l2.id = b2.library_id
                  LEFT JOIN metadata_overrides mo2 ON mo2.book_uuid = b2.uuid
                 WHERE bsl2.series = s.id
                   AND l2.path IN (SELECT p FROM lib_paths)
                 ORDER BY b2.series_index NULLS LAST, b2.sort, b2.id
                 LIMIT 1) AS primary_author,
               (SELECT b3.accent_color
                  FROM books_series_link bsl3
                  JOIN books b3 ON b3.id = bsl3.book
                  JOIN scan_roots l3 ON l3.id = b3.library_id
                 WHERE bsl3.series = s.id
                   AND l3.path IN (SELECT p FROM lib_paths)
                   AND b3.accent_color IS NOT NULL
                 ORDER BY b3.series_index NULLS LAST, b3.sort, b3.id
                 LIMIT 1) AS accent
        FROM series s
        LEFT JOIN counts c ON c.series_id = s.id
        WHERE EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE bsl.series = s.id
               AND l.path IN (SELECT p FROM lib_paths)
          )
        ORDER BY COALESCE(s.sort, s.name) COLLATE NOCASE ASC
        LIMIT ?
        "#
    )
}

/// Map a `list_series` query row to a [`SeriesSummary`].
fn map_series_row(r: &sqlx::sqlite::SqliteRow) -> SeriesSummary {
    SeriesSummary {
        id: r.get("id"),
        name: r.get("name"),
        sort: r.get("sort"),
        book_count: usize::try_from(r.get::<i64, _>("book_count")).unwrap_or(0),
        primary_author: r.get("primary_author"),
        accent: r.get("accent"),
    }
}

#[cfg(test)]
mod tests;
