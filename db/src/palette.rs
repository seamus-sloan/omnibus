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
    let authors: Vec<PaletteAuthorHit> = sqlx::query(
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
    //
    // Issue #154: the per-series correlated `COUNT(*)` is replaced with a
    // single-pass `effective` membership CTE (scoped to the library up
    // front) — the UNION of (1) canonical `books_series_link` rows whose
    // book has no `series` override and (2) the scalar `overrides.series`
    // string for books that do. Each visible series' count is then a single
    // scan of that union. The override match stays BINARY
    // (`json_extract(...) = s.name`, no COLLATE) exactly as before. The
    // clear-all case (`Some("")`) falls out: it drops the book from arm (1)
    // and the empty string won't equal any real series name in arm (2). The
    // `author_display` subquery below is unchanged (not a count — out of
    // scope for #154). UNION (not ALL) is harmless here (a book has one
    // scalar series override) but keeps the shape uniform with the other
    // sites.
    let series: Vec<PaletteSeriesHit> = sqlx::query(
        r#"
        WITH effective AS (
          SELECT bsl.series AS series_id, NULL AS series_name, bsl.book AS book_id
            FROM books_series_link bsl
            JOIN books b ON b.id = bsl.book
            JOIN libraries l2 ON l2.id = b.library_id
            LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE l2.path = ?1
             AND (mo.book_uuid IS NULL
                  OR json_type(mo.overrides, '$.series') IS NULL)
          UNION
          SELECT NULL AS series_id,
                 json_extract(mo.overrides, '$.series') AS series_name,
                 b.id AS book_id
            FROM books b
            JOIN libraries l2 ON l2.id = b.library_id
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
    let tags: Vec<PaletteTagHit> = sqlx::query(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::list_books;
    use crate::covers::test_helpers::CoversTempDir;
    use crate::metadata_overrides::upsert_metadata_overrides;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::sync::test_helpers::indexed;
    use omnibus_shared::{Contributor, MetadataOverrides};
    use sqlx::SqlitePool;

    // ── search_palette ──────────────────────────────────────────────
    #[tokio::test]
    async fn palette_books_match_title() {
        let _covers = CoversTempDir::new("palette_books");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["Horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["Horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "dracula").await.unwrap();
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Dracula");
        assert_eq!(results.books[0].author_display, "Bram Stoker");
        assert!(results.books[0].formats.contains(&"EPUB".to_string()));
    }
    #[tokio::test]
    async fn palette_authors_match_substring() {
        let _covers = CoversTempDir::new("palette_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Babel"),
                &["R. F. Kuang"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "kuang").await.unwrap();
        assert!(!results.authors.is_empty(), "should match author substring");
        assert_eq!(results.authors[0].name, "R. F. Kuang");
        assert_eq!(results.authors[0].book_count, 1);
    }
    #[tokio::test]
    async fn palette_series_match() {
        let _covers = CoversTempDir::new("palette_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Book Two"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "poppy").await.unwrap();
        assert!(!results.series.is_empty(), "should match series substring");
        assert_eq!(results.series[0].name, "Poppy War");
        assert_eq!(results.series[0].book_count, 2);
    }
    #[tokio::test]
    async fn palette_tags_match() {
        let _covers = CoversTempDir::new("palette_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "academia").await.unwrap();
        assert!(!results.tags.is_empty(), "should match tag substring");
        assert_eq!(results.tags[0].name, "Dark academia");
        assert_eq!(results.tags[0].book_count, 1);
    }
    /// Bug #1 (display side): the palette must show the overridden title,
    /// not the canonical scanned `b.title`, so what the user clicks matches
    /// what they searched for.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_title() {
        let _covers = CoversTempDir::new("palette_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Edited Title".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(palette.books[0].title, "Edited Title");
    }
    /// Bug #1 (display side): overriding the creators list rebuilds the
    /// comma-joined `author_display` so the palette subtitle matches the
    /// detail page.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_author_display() {
        let _covers = CoversTempDir::new("palette_override_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Searchable"),
                &["Original Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                creators: Some(vec![
                    Contributor {
                        name: "First Override".into(),
                        ..Default::default()
                    },
                    Contributor {
                        name: "Second Override".into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Searchable").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].author_display,
            "First Override, Second Override"
        );
    }
    /// Palette book hits should surface a user-uploaded cover even when the
    /// scanned book had no cover. Mirrors `apply_overrides` so the palette
    /// row doesn't go cover-less for an override-only cover.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_cover() {
        let _covers = CoversTempDir::new("palette_override_cover");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Indexed book with no scanned cover.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Coverless Searchable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let book = list_books(&pool, "/lib").await.unwrap().remove(0);
        let uuid = book.unique_identifier.clone().unwrap();

        // Set has_cover_override = true with no text edits.
        upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Coverless").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].cover_url,
            Some(format!("/api/covers/{uuid}"))
        );
    }
    // #128: lock the wiring between the palette and `build_fts_match`'s
    // facet prefixes. A regression in the facet parser could otherwise
    // silently break palette tag:/author:/series: queries without any
    // palette test failing.
    #[tokio::test]
    async fn palette_book_matches_tag_facet() {
        let _covers = CoversTempDir::new("palette_tag_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["vampires"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["monsters"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "tag:vampires").await.unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "tag:vampires should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "tag:vampires should not match Frankenstein"
        );
    }
    #[tokio::test]
    async fn palette_book_matches_author_facet() {
        let _covers = CoversTempDir::new("palette_author_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author:stoker")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "author:stoker should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "author:stoker should not match Frankenstein"
        );
    }
    #[tokio::test]
    async fn palette_book_matches_series_facet() {
        let _covers = CoversTempDir::new("palette_series_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author A"],
                    &[],
                    Some(("Dracula Chronicles", "1")),
                    None,
                ),
                indexed("b.epub", Some("Unrelated"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "series:dracula")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Book One"),
            "series:dracula should match Book One, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Unrelated"),
            "series:dracula should not match Unrelated"
        );
    }
    #[tokio::test]
    async fn palette_empty_query_returns_empty() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let results = search_palette(&pool, "/lib", "   ").await.unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
        assert_eq!(results.query, "");
    }
    #[tokio::test]
    async fn palette_no_results() {
        let _covers = CoversTempDir::new("palette_no_results");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "zzzznonexistent")
            .await
            .unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
    }
    #[tokio::test]
    async fn palette_scoped_to_library() {
        let _covers = CoversTempDir::new("palette_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Alpha"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b.epub",
                Some("Beta"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib-a", "tolkien").await.unwrap();
        // Books should only include lib-a
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Alpha");
        // Author book_count should be 1 (scoped to lib-a), not 2
        assert_eq!(results.authors[0].book_count, 1);
    }
    #[tokio::test]
    async fn palette_duration_populated() {
        let _covers = CoversTempDir::new("palette_duration");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author").await.unwrap();
        // duration_ms should be populated (at least 0 — we just check it's set)
        assert!(results.duration_ms < 10000, "duration should be reasonable");
    }
    /// #127 regression coverage: after collapsing the correlated
    /// `book_count` / `EXISTS` subqueries into a single JOIN+GROUP BY,
    /// `l.path = ?1` must still be applied **before** the aggregate so the
    /// scoped library's count doesn't pick up rows from sibling libraries.
    /// This exercises all three taxonomies (authors, series, tags) plus
    /// ordering — the seeded set has 3 matching books in /lib-a and 2 in
    /// /lib-b for the same author/series/tag, and the rare "Sole" author
    /// only appears in /lib-b, so it must be absent from /lib-a results.
    #[tokio::test]
    async fn palette_taxonomy_counts_scoped_per_library() {
        let _covers = CoversTempDir::new("palette_taxonomy_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed(
                    "a1.epub",
                    Some("Alpha One"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "a2.epub",
                    Some("Alpha Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
                indexed(
                    "a3.epub",
                    Some("Alpha Three"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "3")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![
                indexed(
                    "b1.epub",
                    Some("Beta One"),
                    &["Shared Author", "Sole Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "b2.epub",
                    Some("Beta Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Scope to /lib-a — author/series/tag counts must be 3, not 5.
        let results = search_palette(&pool, "/lib-a", "Shared").await.unwrap();

        let author = results
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-a results");
        assert_eq!(
            author.book_count, 3,
            "author count must be scoped to /lib-a, got {results:?}"
        );
        assert!(
            !results.authors.iter().any(|a| a.name == "Sole Author"),
            "Sole Author lives only in /lib-b and must not appear"
        );

        let series = results
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-a results");
        assert_eq!(
            series.book_count, 3,
            "series count must be scoped to /lib-a"
        );

        let tag = results
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-a results");
        assert_eq!(tag.book_count, 3, "tag count must be scoped to /lib-a");

        // Cross-check /lib-b counts to make sure the same query returns 2.
        let results_b = search_palette(&pool, "/lib-b", "Shared").await.unwrap();
        let author_b = results_b
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-b results");
        assert_eq!(author_b.book_count, 2);
        let series_b = results_b
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-b results");
        assert_eq!(series_b.book_count, 2);
        let tag_b = results_b
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-b results");
        assert_eq!(tag_b.book_count, 2);
    }
    #[tokio::test]
    async fn palette_author_count_reflects_overrides() {
        // F5.1: the palette author count must match the merged
        // (override-aware) view, not the raw `books_authors_link` count.
        // Repro of the "Sanderson, Brandon still says 4 books" report:
        // every canonical book for an author was reassigned to a
        // differently-named author through the metadata edit form, so the
        // palette must report 0 books for the source name and the full
        // count for the destination name.
        let _covers = CoversTempDir::new("palette_author_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Two books canonically by "Last, First", plus one book by the
        // already-correct "First Last" so the destination author has a
        // canonical anchor (palette visibility requires ≥1 canonical link).
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Last, First"], &[], None, None),
                indexed("b.epub", Some("B"), &["Last, First"], &[], None, None),
                indexed("c.epub", Some("C"), &["First Last"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // User edits a.epub and b.epub through the metadata form to
        // rename their author to "First Last" — overrides only, no
        // change to the relational link table.
        let books = list_books(&pool, "/lib").await.unwrap();
        for filename in ["a.epub", "b.epub"] {
            let book = books.iter().find(|b| b.filename == filename).unwrap();
            let uuid = book.unique_identifier.clone().unwrap();
            let ov = MetadataOverrides {
                creators: Some(vec![Contributor {
                    name: "First Last".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                }]),
                ..Default::default()
            };
            upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
                .await
                .unwrap();
        }

        let results = search_palette(&pool, "/lib", "Last").await.unwrap();

        // Source author still visible (canonical anchor remains), but
        // count must reflect the effective view: 0 books.
        let source = results
            .authors
            .iter()
            .find(|a| a.name == "Last, First")
            .expect("source author still appears in palette");
        assert_eq!(
            source.book_count, 0,
            "renamed-away author must report effective count 0, got {results:?}",
        );

        // Destination author picks up the override-renamed books on top
        // of its own canonical anchor: 1 + 2 = 3.
        let dest = results
            .authors
            .iter()
            .find(|a| a.name == "First Last")
            .expect("destination author present");
        assert_eq!(
            dest.book_count, 3,
            "destination author must include override-renamed books, got {results:?}",
        );
    }
    #[tokio::test]
    async fn palette_tag_count_reflects_overrides() {
        // F5.1: same shape for tags. `overrides.subjects` replaces the
        // canonical tag list wholesale, so a book moved between tags
        // must shift both counts.
        let _covers = CoversTempDir::new("palette_tag_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["tag-source"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["tag-source"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["tag-dest"], None, None),
            ],
        )
        .await
        .unwrap();

        // Move a.epub off tag-source and onto tag-dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["tag-dest".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "tag-").await.unwrap();
        let source = results
            .tags
            .iter()
            .find(|t| t.name == "tag-source")
            .expect("tag-source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "tag-source should drop a.epub after override, got {results:?}",
        );
        let dest = results
            .tags
            .iter()
            .find(|t| t.name == "tag-dest")
            .expect("tag-dest present");
        assert_eq!(
            dest.book_count, 2,
            "tag-dest should add the override-tagged a.epub, got {results:?}",
        );
    }
    #[tokio::test]
    async fn palette_series_count_reflects_overrides() {
        // F5.1: same shape as palette_author_count_reflects_overrides
        // but for the series tile. Books moved into a series via
        // `overrides.series` must add to the destination count; books
        // moved out drop from the source count.
        let _covers = CoversTempDir::new("palette_series_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["X"],
                    &[],
                    Some(("Series Source", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["X"],
                    &[],
                    Some(("Series Source", "2")),
                    None,
                ),
                indexed(
                    "c.epub",
                    Some("C"),
                    &["X"],
                    &[],
                    Some(("Series Dest", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Move a.epub from Series Source to Series Dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            series: Some("Series Dest".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // "Series" matches both names.
        let results = search_palette(&pool, "/lib", "Series").await.unwrap();
        let source = results
            .series
            .iter()
            .find(|s| s.name == "Series Source")
            .expect("Series Source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "Series Source should count only b.epub after a.epub is overridden away, got {results:?}",
        );
        let dest = results
            .series
            .iter()
            .find(|s| s.name == "Series Dest")
            .expect("Series Dest present");
        assert_eq!(
            dest.book_count, 2,
            "Series Dest should count its canonical c.epub plus the override-moved a.epub, got {results:?}",
        );
    }
    #[tokio::test]
    async fn palette_series_author_display_reflects_override() {
        // F5.1: the "by X" line on a series tile must follow the first
        // book's effective creator, not the canonical one — otherwise
        // renaming the author through the metadata edit form leaves the
        // palette showing the old name.
        let _covers = CoversTempDir::new("palette_series_author_display");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "k1.epub",
                Some("K1"),
                &["Old Name"],
                &[],
                Some(("Kingsway", "1")),
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "New Name".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "Kingsway").await.unwrap();
        let kingsway = results
            .series
            .iter()
            .find(|s| s.name == "Kingsway")
            .expect("Kingsway present");
        assert_eq!(
            kingsway.author_display.as_deref(),
            Some("New Name"),
            "palette author line must follow override.creators, got {results:?}",
        );
    }
    /// #127: capture `EXPLAIN QUERY PLAN` for each of the three rewritten
    /// taxonomy queries and assert the planner uses the link-table indexes.
    /// This is a structural check — it doesn't pin the literal plan string
    /// (SQLite's wording can shift across point releases) but it does fail
    /// loudly if any of the link tables fall back to a full SCAN, which
    /// would defeat the whole point of this optimization.
    #[tokio::test]
    async fn palette_taxonomy_query_plans_use_indexes() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        async fn plan_text(pool: &SqlitePool, sql: &str) -> String {
            let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
                .bind("/lib")
                .bind("%x%")
                .bind(5_i32)
                .fetch_all(pool)
                .await
                .unwrap();
            rows.iter()
                .map(|r| r.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        // Authors — single-pass effective-membership CTE (issue #154).
        // Canonical arm (1) of the union must still drive through the
        // `books_authors_link` index (the library-scoped join), not a full
        // scan, and the visibility `EXISTS` must too.
        let plan = plan_text(
            &pool,
            "WITH effective AS ( \
               SELECT bal.author AS author_id, NULL AS author_name, bal.book AS book_id \
                 FROM books_authors_link bal \
                 JOIN books b ON b.id = bal.book \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND (mo.book_uuid IS NULL OR json_type(mo.overrides, '$.creators') IS NULL) \
               UNION \
               SELECT NULL AS author_id, json_extract(je.value, '$.name') AS author_name, b.id AS book_id \
                 FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                 JOIN json_each(mo.overrides, '$.creators') je \
                WHERE l2.path = ?1 AND json_type(mo.overrides, '$.creators') IS NOT NULL \
             ) \
             SELECT a.id, a.name, \
               (SELECT COUNT(*) FROM effective e \
                 WHERE e.author_id = a.id OR e.author_name = a.name) AS book_count \
             FROM authors a \
             WHERE a.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_authors_link bal \
                             JOIN books b ON b.id = bal.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bal.author = a.id AND l.path = ?1) \
             ORDER BY book_count DESC, a.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_authors_link") && !plan.contains("SCAN bal"),
            "authors plan should not full-scan the link table:\n{plan}"
        );

        // Series — single-pass effective-membership CTE (issue #154).
        // Canonical arm (1) and the visibility `EXISTS` must still drive
        // through the `books_series_link` index, not a full scan.
        let plan = plan_text(
            &pool,
            "WITH effective AS ( \
               SELECT bsl.series AS series_id, NULL AS series_name, bsl.book AS book_id \
                 FROM books_series_link bsl \
                 JOIN books b ON b.id = bsl.book \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND (mo.book_uuid IS NULL OR json_type(mo.overrides, '$.series') IS NULL) \
               UNION \
               SELECT NULL AS series_id, json_extract(mo.overrides, '$.series') AS series_name, b.id AS book_id \
                 FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 AND json_type(mo.overrides, '$.series') IS NOT NULL \
             ) \
             SELECT s.id, s.name, \
               (SELECT COUNT(*) FROM effective e \
                 WHERE e.series_id = s.id OR e.series_name = s.name) AS book_count \
             FROM series s \
             WHERE s.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_series_link bsl \
                             JOIN books b ON b.id = bsl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bsl.series = s.id AND l.path = ?1) \
             ORDER BY book_count DESC, s.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_series_link") && !plan.contains("SCAN bsl"),
            "series plan should not full-scan the link table:\n{plan}"
        );

        // Tags — single-pass effective-membership CTE (issue #154).
        // Canonical arm (1) and the visibility `EXISTS` must still drive
        // through the `books_tags_link` index, not a full scan.
        let plan = plan_text(
            &pool,
            "WITH effective AS ( \
               SELECT btl.tag AS tag_id, NULL AS tag_name, btl.book AS book_id \
                 FROM books_tags_link btl \
                 JOIN books b ON b.id = btl.book \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND (mo.book_uuid IS NULL OR json_type(mo.overrides, '$.subjects') IS NULL) \
               UNION \
               SELECT NULL AS tag_id, je.value AS tag_name, b.id AS book_id \
                 FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                 JOIN json_each(mo.overrides, '$.subjects') je \
                WHERE l2.path = ?1 AND json_type(mo.overrides, '$.subjects') IS NOT NULL \
             ) \
             SELECT t.id, t.name, \
               (SELECT COUNT(*) FROM effective e \
                 WHERE e.tag_id = t.id OR e.tag_name = t.name) AS book_count \
             FROM tags t \
             WHERE t.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_tags_link btl \
                             JOIN books b ON b.id = btl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE btl.tag = t.id AND l.path = ?1) \
             ORDER BY book_count DESC, t.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_tags_link") && !plan.contains("SCAN btl"),
            "tags plan should not full-scan the link table:\n{plan}"
        );
    }
}
