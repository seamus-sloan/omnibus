//! F1.5 search palette: grouped command-palette results across books,
//! authors, series, and tags. Books go through the FTS5 MATCH path
//! (with override-aware overlays applied after hydration); the taxonomy
//! categories use scoped `LIKE` substring matches against the name
//! columns. All results are bounded per category and scoped to
//! `library_path`.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{
    PaletteAuthorHit, PaletteBookHit, PaletteResults, PaletteSeriesHit, PaletteTagHit,
};

use crate::books::parse_json_array;
use crate::helpers::build_fts_match;
use crate::metadata_overrides::load_overrides_bulk;

/// Maximum query length (in chars) accepted by `search_palette`. Inputs
/// beyond this are truncated to bound FTS5 expression size and LIKE
/// pattern length.
const MAX_QUERY_LEN: usize = 256;

/// Search palette — grouped results for the command-palette overlay (F1.5).
///
/// Returns up to 5 results per category (books, authors, series, tags),
/// with server-side timing in `duration_ms`. Books are matched via FTS5
/// (`build_fts_match`); taxonomy categories use `LIKE '%q%'` against the
/// name column. All results are scoped to `library_path`.
///
/// Returns `PaletteResults::default()` for empty/whitespace queries.
pub async fn search_palette(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<PaletteResults, sqlx::Error> {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Ok(PaletteResults::default());
    }
    // Truncate to cap FTS5 + LIKE expression size. Collect chars to
    // avoid slicing mid-codepoint.
    let trimmed: String = trimmed.chars().take(MAX_QUERY_LEN).collect();
    let trimmed = trimmed.as_str();

    let start = std::time::Instant::now();
    const LIMIT: i32 = 5;

    // A. Books — FTS5 MATCH with BM25 ranking, slim projection. After
    // hydration we overlay metadata_overrides so the title and author line
    // shown in the palette match the merged values the rest of the app
    // displays (FTS already matches on the merged text — see
    // `rebuild_fts_for_book` in the override write path).
    let books = if let Some(match_expr) = build_fts_match(trimmed) {
        let rows = sqlx::query(
            r#"
            SELECT b.id, b.uuid, b.title, b.has_cover, b.accent_color,
                   SUBSTR(b.pubdate, 1, 4) AS year,

                   (SELECT GROUP_CONCAT(a.name, ', ')
                      FROM (SELECT a2.name FROM books_authors_link bal
                              JOIN authors a2 ON a2.id = bal.author
                             WHERE bal.book = b.id
                             ORDER BY bal.position) a)          AS author_display,

                   (SELECT json_group_array(format)
                      FROM (SELECT format FROM book_files
                             WHERE book_id = b.id
                             ORDER BY format))                  AS formats_json

            FROM books_fts
            JOIN books b ON b.id = books_fts.rowid
            JOIN libraries l ON l.id = b.library_id
            WHERE books_fts MATCH ?1 AND l.path = ?2
            ORDER BY bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0), b.sort, b.id
            LIMIT ?3
            "#,
        )
        .bind(&match_expr)
        .bind(library_path)
        .bind(LIMIT)
        .fetch_all(pool)
        .await?;

        let mut uuids: Vec<String> = Vec::with_capacity(rows.len());
        let mut hits: Vec<PaletteBookHit> = Vec::with_capacity(rows.len());
        for r in rows.iter() {
            let id: i64 = r.get("id");
            let uuid: String = r.get("uuid");
            let has_cover: i64 = r.get("has_cover");
            uuids.push(uuid.clone());
            hits.push(PaletteBookHit {
                id,
                uuid: uuid.clone(),
                title: r.get::<Option<String>, _>("title").unwrap_or_default(),
                author_display: r
                    .get::<Option<String>, _>("author_display")
                    .unwrap_or_default(),
                year: r.get("year"),
                formats: parse_json_array(r.get("formats_json"))?,
                cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
                accent: r.get("accent_color"),
            });
        }

        let overrides_map = load_overrides_bulk(pool, &uuids).await?;
        for (hit, uuid) in hits.iter_mut().zip(uuids.iter()) {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                if let Some(ref t) = ov.title {
                    hit.title = t.clone();
                }
                if let Some(ref creators) = ov.creators {
                    hit.author_display = creators
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                }
                // Mirror `apply_overrides`: surface user-uploaded covers even
                // when the scanned book had `has_cover = 0`.
                if *has_cover_ov {
                    hit.cover_url = Some(format!("/api/covers/{}", hit.uuid));
                }
            }
        }

        hits
    } else {
        Vec::new()
    };

    // Escape the query for LIKE pattern: backslash first (it's the ESCAPE char),
    // then the LIKE wildcards percent and underscore.
    let like_q = trimmed
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let like_pattern = format!("%{like_q}%");

    // B. Authors — substring match, scoped to library, ordered by book count.
    //
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
    let authors: Vec<PaletteAuthorHit> = sqlx::query(
        r#"
        SELECT a.id, a.name,
          (SELECT COUNT(*)
             FROM books b
             JOIN libraries l2 ON l2.id = b.library_id
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            WHERE l2.path = ?1
              AND CASE
                    WHEN mo.book_uuid IS NOT NULL
                         AND json_type(mo.overrides, '$.creators') IS NOT NULL
                      THEN EXISTS (
                        SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                         WHERE json_extract(je.value, '$.name') = a.name
                      )
                    ELSE EXISTS (
                      SELECT 1 FROM books_authors_link bal
                       WHERE bal.book = b.id AND bal.author = a.id
                    )
                  END
          ) AS book_count
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
    .bind(&like_pattern)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| PaletteAuthorHit {
        id: r.get("id"),
        name: r.get("name"),
        book_count: r.get::<i32, _>("book_count") as u32,
    })
    .collect();

    // C. Series — substring match with primary author from first book.
    //
    // F5.1: both the count and the `author_display` line use the effective
    // (override-aware) view, mirroring `get_series` and the palette author
    // count. `overrides.series` (string) drives membership; if a book's
    // first creator was renamed through the metadata edit form,
    // `overrides.creators[0].name` drives the displayed author. Visibility
    // still requires at least one canonical link in this library so we
    // don't list series that exist only inside override JSON (no
    // navigable id).
    let series: Vec<PaletteSeriesHit> = sqlx::query(
        r#"
        SELECT s.id, s.name,
          (SELECT COUNT(*)
             FROM books b
             JOIN libraries l2 ON l2.id = b.library_id
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            WHERE l2.path = ?1
              AND CASE
                    WHEN mo.book_uuid IS NOT NULL
                         AND json_type(mo.overrides, '$.series') IS NOT NULL
                      THEN json_extract(mo.overrides, '$.series') = s.name
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
            WHERE bsl2.series = s.id AND l2.path = ?1
            ORDER BY b2.sort, b2.id LIMIT 1) AS author_display
        FROM series s
        WHERE s.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bsl.series = s.id AND l.path = ?1
          )
        ORDER BY book_count DESC, s.name
        LIMIT ?3
        "#,
    )
    .bind(library_path)
    .bind(&like_pattern)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| PaletteSeriesHit {
        id: r.get("id"),
        name: r.get("name"),
        book_count: r.get::<i32, _>("book_count") as u32,
        author_display: r.get("author_display"),
    })
    .collect();

    // D. Tags — substring match, scoped to library.
    //
    // F5.1: the count uses the effective (override-aware) subject set,
    // not the raw `books_tags_link` rows. `MetadataOverrides.subjects`
    // (Option<Vec<String>>) replaces the canonical tag list wholesale
    // when Some — including the empty array, which clears all tags.
    // Visibility still requires at least one canonical link in this
    // library so we don't list tags that exist only inside override
    // JSON (no navigable id).
    let tags: Vec<PaletteTagHit> = sqlx::query(
        r#"
        SELECT t.id, t.name,
          (SELECT COUNT(*)
             FROM books b
             JOIN libraries l2 ON l2.id = b.library_id
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            WHERE l2.path = ?1
              AND CASE
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
          ) AS book_count
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
    .bind(&like_pattern)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| PaletteTagHit {
        id: r.get("id"),
        name: r.get("name"),
        book_count: r.get::<i32, _>("book_count") as u32,
    })
    .collect();

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(PaletteResults {
        query: trimmed.to_string(),
        books,
        authors,
        series,
        tags,
        duration_ms,
    })
}
