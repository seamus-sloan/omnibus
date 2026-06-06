//! Authors arm of the search palette: substring `LIKE` match scoped to a
//! library, ordered by an override-aware effective book count. Visibility
//! still requires at least one canonical link so override-only authors
//! (no navigable id) don't appear.

use sqlx::{Row, SqlitePool};

use omnibus_shared::PaletteAuthorHit;

use super::PaletteError;

/// Run the authors arm of the palette for `like_pattern` (already escaped)
/// scoped to `library_path`, capped to `limit`.
pub async fn search_authors(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteAuthorHit>, PaletteError> {
    // Library scoping (`l.path = ?1`) is applied before aggregation so
    // book_count stays library-correct (covered by `palette_scoped_to_library`
    // and `palette_taxonomy_counts_scoped_per_library`). The join plan is
    // locked in by `palette_taxonomy_query_plans_use_indexes`.
    //
    // F5.1: the count uses the effective (override-aware) creator set, not
    // the raw `books_authors_link` rows — otherwise an author whose books
    // were all reassigned through the metadata edit form (e.g. "Sanderson,
    // Brandon" → "Brandon Sanderson") keeps reporting the canonical count
    // even though `/author/:id` shows zero. Visibility still requires at
    // least one canonical link row in this library so we don't list
    // authors that exist only as a string inside override JSON (no
    // navigable id), matching the rest of the palette's behavior.
    //
    // Issue #154: the per-author correlated `COUNT(*)` is replaced with a
    // single-pass `effective` membership CTE (scoped to the library up
    // front) — the UNION of (1) canonical `books_authors_link` rows whose
    // book has no `creators` override and (2) override-extracted creator
    // names from `json_each(mo.overrides, '$.creators')`. Each visible
    // author's count is then a single scan of that union. UNION (not ALL)
    // dedupes a creator repeated within one override array, matching the
    // prior `EXISTS` semantics. The override name match stays BINARY
    // (`= a.name`, no COLLATE) exactly as before. The empty-array clear-all
    // case falls out: a `Some([])` override drops the book from arm (1) and
    // yields no `json_each` rows in arm (2).
    let rows = sqlx::query(
        r#"
        WITH effective AS (
          SELECT bal.author AS author_id, NULL AS author_name, bal.book AS book_id
            FROM books_authors_link bal
            JOIN books b ON b.id = bal.book
            JOIN libraries l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE l2.path = ?1
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.creators') IS NULL)
          UNION
          SELECT NULL AS author_id,
                 json_extract(je.value, '$.name') AS author_name,
                 b.id AS book_id
            FROM books b
            JOIN libraries l2 ON l2.id = b.library_id
            JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            JOIN json_each(mo.overrides, '$.creators') je
           WHERE l2.path = ?1
             AND json_type(mo.overrides, '$.creators') IS NOT NULL
        )
        SELECT a.id, a.name,
          (SELECT COUNT(*) FROM effective e
            WHERE e.author_id = a.id OR e.author_name = a.name) AS book_count
        FROM authors a
        WHERE a.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bal.author = a.id AND l.path = ?1
          )
        ORDER BY book_count DESC, a.name
        LIMIT ?3
        "#,
    )
    .bind(library_path)
    .bind(like_pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| PaletteAuthorHit {
            id: r.get("id"),
            name: r.get("name"),
            book_count: r.get::<i32, _>("book_count") as u32,
        })
        .collect())
}
