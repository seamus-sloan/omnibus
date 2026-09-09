//! Authors arm of the search palette: substring `LIKE` match scoped to the
//! visible books, ordered by an override-aware effective book count.
//! Visibility is the rule `browse::list_authors` uses — membership in that
//! same effective set — so the palette cannot offer an author the Authors
//! index rejects.

use std::sync::OnceLock;

use omnibus_shared::PaletteAuthorHit;
use sqlx::{Row, SqlitePool};

use crate::helpers::{library_paths_json, visible_book_sql};

use super::PaletteError;

/// Authors-arm palette query, bound `?1 = library_paths JSON array`, `?2 = like_pattern`,
/// `?3 = limit`.
///
/// Visibility scoping ([`visible_book_sql`]) is applied before aggregation so
/// book_count stays library-correct (covered by
/// `search_palette_scoped_to_library` and
/// `search_palette_taxonomy_counts_scoped_per_library`). The join plan is
/// locked in by `search_palette_taxonomy_query_plans_use_indexes`.
///
/// The count uses the effective (override-aware) creator set, not the
/// raw `books_authors_link` rows — otherwise an author whose books were all
/// reassigned through the metadata edit form (e.g. "Sanderson, Brandon" →
/// "Brandon Sanderson") keeps reporting the canonical count even though
/// `/author/:id` shows zero. Visibility still requires at least one canonical
/// link row on a visible book so we don't list authors that exist only as a
/// string inside override JSON (no navigable id), matching the rest of the
/// palette's behavior.
///
/// The per-author correlated `COUNT(*)` is replaced with a
/// single-pass `effective` membership CTE (scoped to the visible books up
/// front) —
/// the UNION of (1) canonical `books_authors_link` rows whose book has no
/// `creators` override and (2) override-extracted creator names from
/// `json_each(mo.overrides, '$.creators')`. Each visible author's count is
/// then a single scan of that union. UNION (not ALL) dedupes a creator
/// repeated within one override array, matching the prior `EXISTS` semantics.
/// The override name match stays BINARY (`= a.name`, no COLLATE) exactly as
/// before. The empty-array clear-all case falls out: a `Some([])` override
/// drops the book from arm (1) and yields no `json_each` rows in arm (2).
pub(super) fn search_authors_sql() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        let vis = visible_book_sql("b", "l2", "?1");
        format!(
            r"
        WITH effective AS (
          -- (1) Canonical authorship on a book with no creators override.
          SELECT bal.author AS author_id, bal.book AS book_id
            FROM books_authors_link bal
            JOIN books b ON b.id = bal.book
            JOIN scan_roots l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE {vis}
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.creators') IS NULL)
          UNION
          -- (2) Override creators resolved (NOCASE) to an authors row, so the
          -- credit lands on the id the reader's links point at. Migration
          -- 0096 and `materialize_author_rows` guarantee the row exists.
          SELECT a2.id AS author_id, b.id AS book_id
            FROM books b
            JOIN scan_roots l2 ON l2.id = b.library_id
            JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            JOIN json_each(mo.overrides, '$.creators') je
            JOIN authors a2
              ON a2.name = json_extract(je.value, '$.name') COLLATE NOCASE
           WHERE {vis}
             AND json_type(mo.overrides, '$.creators') IS NOT NULL
        ),
        counts AS (
          SELECT author_id, COUNT(*) AS book_count
            FROM effective
           GROUP BY author_id
        )
        SELECT a.id, a.name,
          -- `JOIN counts` below is what enforces book_count > 0: a bare
          -- canonical-link EXISTS kept a scanned row whose books had all been
          -- re-credited, and it opened onto an empty author page (#2502).
          c.book_count AS book_count,
          -- The lead title comes off the effective set too: reading it from
          -- `books_authors_link` is what let a dead row advertise `incl. Six
          -- of Crows` for a book it no longer credits.
          (SELECT COALESCE(json_extract(mo3.overrides, '$.title'), b3.title)
             FROM effective e3
             JOIN books b3 ON b3.id = e3.book_id
             LEFT JOIN metadata_overrides mo3 ON mo3.book_uuid = b3.uuid
            WHERE e3.author_id = a.id
            ORDER BY b3.sort, b3.id LIMIT 1) AS lead_book_title
        FROM authors a
        JOIN counts c ON c.author_id = a.id
        WHERE a.name LIKE ?2 ESCAPE '\'
        ORDER BY book_count DESC, a.name
        LIMIT ?3
        "
        )
    })
}

/// Run the authors arm of the palette for `like_pattern` (already escaped)
/// scoped to `library_path`, capped to `limit`.
pub async fn search_authors(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteAuthorHit>, PaletteError> {
    search_authors_for_paths(pool, &[library_path], like_pattern, limit).await
}

/// Run the authors arm across every configured library path.
pub async fn search_authors_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    like_pattern: &str,
    limit: i32,
) -> Result<Vec<PaletteAuthorHit>, PaletteError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(search_authors_sql())
        .bind(library_paths_json(library_paths))
        .bind(like_pattern)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .iter()
        .map(|r| PaletteAuthorHit {
            id: r.get("id"),
            name: r.get("name"),
            book_count: u32::try_from(r.get::<i32, _>("book_count")).unwrap_or(0),
            lead_book_title: r.get("lead_book_title"),
        })
        .collect())
}

/// Count visible authors matching `like_pattern` in `library_path` — the
/// uncapped total behind the palette's 5-hit author cap. "Visible" mirrors
/// [`search_authors`]: the author needs at least one canonical link on a
/// visible book (override-only names have no navigable id and are excluded).
pub async fn count_authors(
    pool: &SqlitePool,
    library_path: &str,
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    count_authors_for_paths(pool, &[library_path], like_pattern).await
}

/// Count visible matching authors across every configured library path.
pub async fn count_authors_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    like_pattern: &str,
) -> Result<i64, PaletteError> {
    if library_paths.is_empty() {
        return Ok(0);
    }
    let vis = visible_book_sql("b", "l2", "?1");
    Ok(sqlx::query_scalar::<_, i64>(&format!(
        r"
        WITH effective AS (
          SELECT bal.author AS author_id, bal.book AS book_id
            FROM books_authors_link bal
            JOIN books b ON b.id = bal.book
            JOIN scan_roots l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE {vis}
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.creators') IS NULL)
          UNION
          SELECT a2.id AS author_id, b.id AS book_id
            FROM books b
            JOIN scan_roots l2 ON l2.id = b.library_id
            JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            JOIN json_each(mo.overrides, '$.creators') je
            JOIN authors a2
              ON a2.name = json_extract(je.value, '$.name') COLLATE NOCASE
           WHERE {vis}
             AND json_type(mo.overrides, '$.creators') IS NOT NULL
        )
        SELECT COUNT(*) FROM authors a
        WHERE a.name LIKE ?2 ESCAPE '\'
          AND EXISTS (SELECT 1 FROM effective e WHERE e.author_id = a.id)
        "
    ))
    .bind(library_paths_json(library_paths))
    .bind(like_pattern)
    .fetch_one(pool)
    .await?)
}
