//! Browse-all index pages: `/authors` and `/series`. Returns every row
//! (capped at `INDEX_LIMIT`) so the UI's client-side sort/filter has the
//! full list to work with; per-row counts come back override-aware so the
//! index stays consistent with the discovery-detail reads. Single-tenant
//! today — no per-user ACL filtering.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{AuthorSummary, SeriesSummary};

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
) -> Result<Vec<AuthorSummary>, sqlx::Error> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let ph = placeholders(library_paths.len());
    let sql = format!(
        r#"
        WITH lib_paths(p) AS (VALUES {ph})
        SELECT a.id, a.name, a.sort,
               (SELECT COUNT(*)
                  FROM books b
                  JOIN libraries l2 ON l2.id = b.library_id
                  LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                 WHERE l2.path IN (SELECT p FROM lib_paths)
                   AND CASE
                         WHEN mo.book_uuid IS NOT NULL
                              AND json_type(mo.overrides, '$.creators') IS NOT NULL
                           THEN EXISTS (
                             SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                              WHERE json_extract(je.value, '$.name') = a.name COLLATE NOCASE
                           )
                         ELSE EXISTS (
                           SELECT 1 FROM books_authors_link bal
                            WHERE bal.book = b.id AND bal.author = a.id
                         )
                       END
               ) AS book_count,
               (SELECT b2.accent_color
                  FROM books_authors_link bal2
                  JOIN books b2 ON b2.id = bal2.book
                  JOIN libraries l2 ON l2.id = b2.library_id
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
        WHERE EXISTS (
            SELECT 1 FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN libraries l ON l.id = b.library_id
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
            book_count: r.get::<i64, _>("book_count") as usize,
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
) -> Result<Vec<SeriesSummary>, sqlx::Error> {
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
/// to materialize the path set once so the four correlated subqueries each see
/// the same `lib_paths` without repeated inline VALUES lists.
fn series_index_sql(n: usize) -> String {
    let ph = placeholders(n);
    format!(
        r#"
        WITH lib_paths(p) AS (VALUES {ph})
        SELECT s.id, s.name, s.sort,
               (SELECT COUNT(*)
                  FROM books b
                  JOIN libraries l2 ON l2.id = b.library_id
                  LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                 WHERE l2.path IN (SELECT p FROM lib_paths)
                   AND CASE
                         WHEN mo.book_uuid IS NOT NULL
                              AND json_type(mo.overrides, '$.series') IS NOT NULL
                           THEN json_extract(mo.overrides, '$.series') = s.name COLLATE NOCASE
                         ELSE EXISTS (
                           SELECT 1 FROM books_series_link bsl
                            WHERE bsl.book = b.id AND bsl.series = s.id
                         )
                       END
               ) AS book_count,
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
                  JOIN libraries l2 ON l2.id = b2.library_id
                  LEFT JOIN metadata_overrides mo2 ON mo2.book_uuid = b2.uuid
                 WHERE bsl2.series = s.id
                   AND l2.path IN (SELECT p FROM lib_paths)
                 ORDER BY b2.series_index NULLS LAST, b2.sort, b2.id
                 LIMIT 1) AS primary_author,
               (SELECT b3.accent_color
                  FROM books_series_link bsl3
                  JOIN books b3 ON b3.id = bsl3.book
                  JOIN libraries l3 ON l3.id = b3.library_id
                 WHERE bsl3.series = s.id
                   AND l3.path IN (SELECT p FROM lib_paths)
                   AND b3.accent_color IS NOT NULL
                 ORDER BY b3.series_index NULLS LAST, b3.sort, b3.id
                 LIMIT 1) AS accent
        FROM series s
        WHERE EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN libraries l ON l.id = b.library_id
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
        book_count: r.get::<i64, _>("book_count") as usize,
        primary_author: r.get("primary_author"),
        accent: r.get("accent"),
    }
}

#[cfg(test)]
mod tests;
