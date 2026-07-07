//! Series arm of the search palette: substring `LIKE` match with
//! override-aware book count plus an author_display drawn from the
//! first book's effective creators. Visibility still requires at least
//! one canonical link in this library.

use omnibus_shared::PaletteSeriesHit;
use sqlx::{Row, SqlitePool};

use super::PaletteError;

/// Series-arm palette query, bound `?1 = library_path`, `?2 = like_pattern`,
/// `?3 = limit`.
///
/// F5.1: both the count and the `author_display` line use the effective
/// (override-aware) view, mirroring `get_series` and the palette author
/// count. `overrides.series` (string) drives membership; if a book's first
/// creator was renamed through the metadata edit form,
/// `overrides.creators[0].name` drives the displayed author. Visibility still
/// requires at least one canonical link in this library so we don't list
/// series that exist only inside override JSON (no navigable id).
///
/// Issue #154: the per-series correlated `COUNT(*)` is replaced with a
/// single-pass `effective` membership CTE (scoped to the library up front) —
/// the UNION of (1) canonical `books_series_link` rows whose book has no
/// `series` override and (2) the scalar `overrides.series` string for books
/// that do. Each visible series' count is then a single scan of that union.
/// The override match stays BINARY (`json_extract(...) = s.name`, no COLLATE)
/// exactly as before. The clear-all case (`Some("")`) falls out: it drops the
/// book from arm (1) and the empty string won't equal any real series name in
/// arm (2). The `author_display` subquery is unchanged (not a count — out of
/// scope for #154). UNION (not ALL) is harmless here (a book has one scalar
/// series override) but keeps the shape uniform with the other sites.
const SEARCH_SERIES_SQL: &str = r"
        WITH effective AS (
          SELECT bsl.series AS series_id, NULL AS series_name, bsl.book AS book_id
            FROM books_series_link bsl
            JOIN books b ON b.id = bsl.book
            JOIN scan_roots l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE l2.path = ?1
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.series') IS NULL)
          UNION
          SELECT NULL AS series_id,
                 json_extract(mo.overrides, '$.series') AS series_name,
                 b.id AS book_id
            FROM books b
            JOIN scan_roots l2 ON l2.id = b.library_id
            JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE l2.path = ?1
             AND json_type(mo.overrides, '$.series') IS NOT NULL
        )
        SELECT s.id, s.name,
          (SELECT COUNT(*) FROM effective e
            WHERE e.series_id = s.id OR e.series_name = s.name) AS book_count,
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
            WHERE bsl2.series = s.id AND l2.path = ?1
            ORDER BY b2.sort, b2.id LIMIT 1) AS author_display,
          (SELECT COALESCE(json_extract(mo3.overrides, '$.title'), b3.title)
             FROM books_series_link bsl3
             JOIN books b3 ON b3.id = bsl3.book
             JOIN scan_roots l3 ON l3.id = b3.library_id
             LEFT JOIN metadata_overrides mo3 ON mo3.book_uuid = b3.uuid
            WHERE bsl3.series = s.id AND l3.path = ?1
            ORDER BY b3.sort, b3.id LIMIT 1) AS lead_book_title
        FROM series s
        WHERE s.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE bsl.series = s.id AND l.path = ?1
          )
        ORDER BY book_count DESC, s.name
        LIMIT ?3
        ";

/// Run the series arm of the palette for `like_pattern` (already escaped)
/// scoped to `library_path`, capped to `limit`.
pub async fn search_series(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteSeriesHit>, PaletteError> {
    let rows = sqlx::query(SEARCH_SERIES_SQL)
        .bind(library_path)
        .bind(like_pattern)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .iter()
        .map(|r| PaletteSeriesHit {
            id: r.get("id"),
            name: r.get("name"),
            book_count: u32::try_from(r.get::<i32, _>("book_count")).unwrap_or(0),
            author_display: r.get("author_display"),
            lead_book_title: r.get("lead_book_title"),
        })
        .collect())
}

/// Count visible series matching `like_pattern` in `library_path` — the
/// uncapped total behind the palette's 5-hit series cap. Visibility mirrors
/// [`search_series`]: at least one canonical link in this library.
pub async fn count_series(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r"
        SELECT COUNT(*) FROM series s
        WHERE s.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE bsl.series = s.id AND l.path = ?1
          )
        ",
    )
    .bind(library_path)
    .bind(like_pattern)
    .fetch_one(pool)
    .await?)
}
