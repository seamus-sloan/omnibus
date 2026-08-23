//! Genres arm of the search palette: substring `LIKE` match scoped to the
//! visible books, ordered by book count. Simpler than
//! [`super::tags`] — genres have no link table, so there is no canonical arm
//! to `UNION` against and the override JSON is the whole story, exactly as in
//! [`crate::discovery::genres`].

use std::sync::OnceLock;

use omnibus_shared::PaletteGenreHit;
use sqlx::{Row, SqlitePool};

use crate::helpers::{library_paths_json, visible_book_sql};

use super::PaletteError;

/// The `FROM`/`WHERE` body both the hits query and the count share, bound
/// `?1 = library_paths JSON array`, `?2 = like_pattern`.
///
/// The join to `genres` — rather than grouping `je.value` directly — is what
/// makes the reported name canonical: `materialize_genre_rows` deduplicates
/// into a `NOCASE`-unique row, so a library holding both "sci-fi" and
/// "Sci-Fi" shows one palette row under whichever spelling was coined first,
/// matching `get_genre_cloud` and the landing facets. Grouping the raw JSON
/// values instead would split them into two rows whose counts each cover half
/// the shelf.
///
/// `COUNT(DISTINCT b.id)` guards a duplicate entry inside a single book's
/// array (`["Horror","Horror"]` counts once).
fn genre_scan_sql() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        let visible = visible_book_sql("b", "l", "?1");
        format!(
            r"
        FROM books b
        JOIN scan_roots l ON l.id = b.library_id
        JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
        JOIN json_each(mo.overrides, '$.genres') je
        JOIN genres g ON g.name = je.value COLLATE NOCASE
       WHERE json_type(mo.overrides, '$.genres') IS NOT NULL
         AND g.name LIKE ?2 ESCAPE '\'
         AND {visible}
        "
        )
    })
}

/// Genres-arm palette query, bound `?1 = library_paths JSON array`,
/// `?2 = like_pattern`, `?3 = limit`. Ordering mirrors `get_genre_cloud`:
/// count descending, then name ascending.
fn search_genres_sql() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        format!(
            "SELECT g.name AS name, COUNT(DISTINCT b.id) AS book_count
             {}
             GROUP BY g.id, g.name
             ORDER BY book_count DESC, g.name
             LIMIT ?3",
            genre_scan_sql()
        )
    })
}

/// Run the genres arm of the palette for `like_pattern` (already escaped)
/// scoped to `library_path`, capped to `limit`.
pub async fn search_genres(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteGenreHit>, PaletteError> {
    search_genres_for_paths(pool, &[library_path], like_pattern, limit).await
}

/// Run the genres arm across every configured library path.
pub async fn search_genres_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteGenreHit>, PaletteError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(search_genres_sql())
        .bind(library_paths_json(library_paths))
        .bind(like_pattern)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .iter()
        .map(|r| PaletteGenreHit {
            name: r.get("name"),
            book_count: u32::try_from(r.get::<i64, _>("book_count")).unwrap_or(0),
        })
        .collect())
}

/// Count visible genres matching `like_pattern` in `library_path` — the
/// uncapped total behind the palette's 5-hit genre cap.
pub async fn count_genres(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    count_genres_for_paths(pool, &[library_path], like_pattern).await
}

/// Count visible matching genres across every configured library path.
///
/// The `GROUP BY` has to be counted from the outside rather than folded into
/// a `COUNT(DISTINCT g.id)`: the scan yields one row per (book, genre) pair,
/// and the total the header wants is how many *genres* matched.
pub async fn count_genres_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    if library_paths.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>(&format!(
        "SELECT COUNT(*) FROM (SELECT g.id {} GROUP BY g.id)",
        genre_scan_sql()
    ))
    .bind(library_paths_json(library_paths))
    .bind(like_pattern)
    .fetch_one(pool)
    .await?)
}
