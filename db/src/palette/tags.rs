//! Tags arm of the search palette: substring `LIKE` match scoped to the
//! visible books, ordered by an override-aware effective book count.
//! Visibility still requires at least one canonical link so override-only
//! tags (no navigable id) don't appear.

use std::sync::OnceLock;

use omnibus_shared::PaletteTagHit;
use sqlx::{Row, SqlitePool};

use crate::helpers::{library_paths_json, visible_book_sql};

use super::PaletteError;

/// Tags-arm palette query, bound `?1 = library_path`, `?2 = like_pattern`,
/// `?3 = limit`.
///
/// The count uses the effective (override-aware) subject set, not the raw
/// `books_tags_link` rows. `MetadataOverrides.subjects` (Option<Vec<String>>)
/// replaces the canonical tag list wholesale when Some — including the empty
/// array, which clears all tags. Visibility still requires at least one
/// canonical link on a visible book so we don't list tags that exist only
/// inside override JSON (no navigable id). `effective` is scoped once up
/// front, but `book_count` is still a per-tag correlated `COUNT(*)` over it.
/// The `e.tag_name = t.name` override-arm match stays BINARY even though
/// `tags.name` is `COLLATE NOCASE`: `tag_name` is a CTE column with no
/// explicit collation, and SQLite's column-collation precedence favors the
/// left operand, so BINARY wins. The empty-array clear-all case falls out
/// naturally since a `Some([])` override drops the book from the canonical
/// arm and yields no `json_each` rows either.
pub(super) fn search_tags_sql() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        let vis = visible_book_sql("b", "l2", "?1");
        let vis_exists = visible_book_sql("b", "l", "?1");
        format!(
            r"
        WITH effective AS (
          SELECT btl.tag AS tag_id, NULL AS tag_name, btl.book AS book_id
            FROM books_tags_link btl
            JOIN books b ON b.id = btl.book
            JOIN scan_roots l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE {vis}
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.subjects') IS NULL)
          UNION
          SELECT NULL AS tag_id, je.value AS tag_name, b.id AS book_id
            FROM books b
            JOIN scan_roots l2 ON l2.id = b.library_id
            JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            JOIN json_each(mo.overrides, '$.subjects') je
           WHERE {vis}
             AND json_type(mo.overrides, '$.subjects') IS NOT NULL
        )
        SELECT t.id, t.name,
          (SELECT COUNT(*) FROM effective e
            WHERE e.tag_id = t.id OR e.tag_name = t.name) AS book_count
        FROM tags t
        WHERE t.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_tags_link btl
              JOIN books b ON b.id = btl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE btl.tag = t.id
               AND {vis_exists}
          )
        ORDER BY book_count DESC, t.name
        LIMIT ?3
        "
        )
    })
}

/// Run the tags arm of the palette for `like_pattern` (already escaped)
/// scoped to `library_path`, capped to `limit`.
pub async fn search_tags(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteTagHit>, PaletteError> {
    search_tags_for_paths(pool, &[library_path], like_pattern, limit).await
}

/// Run the tags arm across every configured library path.
pub async fn search_tags_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteTagHit>, PaletteError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(search_tags_sql())
        .bind(library_paths_json(library_paths))
        .bind(like_pattern)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .iter()
        .map(|r| PaletteTagHit {
            id: r.get("id"),
            name: r.get("name"),
            book_count: u32::try_from(r.get::<i32, _>("book_count")).unwrap_or(0),
        })
        .collect())
}

/// Count visible tags matching `like_pattern` in `library_path` — the
/// uncapped total behind the palette's 5-hit tag cap. Visibility mirrors
/// [`search_tags`]: at least one canonical link on a visible book.
pub async fn count_tags(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    count_tags_for_paths(pool, &[library_path], like_pattern).await
}

/// Count visible matching tags across every configured library path.
pub async fn count_tags_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    if library_paths.is_empty() {
        return Ok(0);
    }
    let visible = visible_book_sql("b", "l", "?1");
    Ok(sqlx::query_scalar::<_, i64>(&format!(
        r"
        SELECT COUNT(*) FROM tags t
        WHERE t.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_tags_link btl
              JOIN books b ON b.id = btl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE btl.tag = t.id
               AND {visible}
          )
        "
    ))
    .bind(library_paths_json(library_paths))
    .bind(like_pattern)
    .fetch_one(pool)
    .await?)
}
