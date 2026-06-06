//! Tags arm of the search palette: substring `LIKE` match scoped to a
//! library, ordered by an override-aware effective book count. Visibility
//! still requires at least one canonical link so override-only tags
//! (no navigable id) don't appear.

use sqlx::{Row, SqlitePool};

use omnibus_shared::PaletteTagHit;

use super::PaletteError;

/// Run the tags arm of the palette for `like_pattern` (already escaped)
/// scoped to `library_path`, capped to `limit`.
pub async fn search_tags(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteTagHit>, PaletteError> {
    // F5.1: the count uses the effective (override-aware) subject set,
    // not the raw `books_tags_link` rows. `MetadataOverrides.subjects`
    // (Option<Vec<String>>) replaces the canonical tag list wholesale
    // when Some — including the empty array, which clears all tags.
    // Visibility still requires at least one canonical link in this
    // library so we don't list tags that exist only inside override
    // JSON (no navigable id).
    //
    // Issue #154: the per-tag correlated `COUNT(*)` is replaced with a
    // single-pass `effective` membership CTE (scoped to the library up
    // front) — the UNION of (1) canonical `books_tags_link` rows whose book
    // has no `subjects` override and (2) override-extracted subject strings
    // from `json_each(mo.overrides, '$.subjects')`. UNION (not ALL) dedupes
    // duplicate subject strings within one override array, matching the
    // prior `EXISTS` semantics. The override match stays BINARY
    // (`je.value = t.name`, no COLLATE). The empty-array clear-all case
    // falls out: a `Some([])` override drops the book from arm (1) and
    // yields no `json_each` rows in arm (2).
    let rows = sqlx::query(
        r#"
        WITH effective AS (
          SELECT btl.tag AS tag_id, NULL AS tag_name, btl.book AS book_id
            FROM books_tags_link btl
            JOIN books b ON b.id = btl.book
            JOIN libraries l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE l2.path = ?1
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.subjects') IS NULL)
          UNION
          SELECT NULL AS tag_id, je.value AS tag_name, b.id AS book_id
            FROM books b
            JOIN libraries l2 ON l2.id = b.library_id
            JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            JOIN json_each(mo.overrides, '$.subjects') je
           WHERE l2.path = ?1
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
              JOIN libraries l ON l.id = b.library_id
             WHERE btl.tag = t.id AND l.path = ?1
          )
        ORDER BY book_count DESC, t.name
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
        .map(|r| PaletteTagHit {
            id: r.get("id"),
            name: r.get("name"),
            book_count: r.get::<i32, _>("book_count") as u32,
        })
        .collect())
}
