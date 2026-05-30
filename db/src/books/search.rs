//! FTS5-backed search read path. Wraps the `books_fts` virtual table with
//! the same scalar-subquery projection the other read paths use, so search
//! results hydrate into the same `EbookMetadata` shape `list_books` /
//! `get_book` return.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use crate::helpers::{build_fts_match, cap_query_len, format_series_index};
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk};

use super::projection::{
    backfill_creator_ids, parse_json_array, sanitize_description, CreatorRow, IdentifierRow,
    MAX_BOOKS_RETURNED,
};

/// Full-text search across `books_fts`. Returns hydrated `EbookMetadata`
/// ordered by bm25 rank (best first). Free-text terms are scoped to
/// `title/authors/series` via a column filter so that short prefix queries
/// don't surface spurious hits on generic `tags` values (e.g. typing "Dr"
/// matching books tagged "Drama"). Ranking weights favour title matches:
/// `bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0)` — unused columns keep
/// neutral weights since the column filter prevents them from contributing.
///
/// `q` is parsed via [`build_fts_match`] (which recognises `author:`,
/// `series:`, `tag:` facets and sanitises every token) before reaching
/// `MATCH`, so arbitrary user input is safe to pass through. Returns an
/// empty vec when the parsed query is empty.
pub async fn search_books(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
    let (books, _total) = search_books_with_total(pool, library_path, q).await?;
    Ok(books)
}

/// Same as [`search_books`] but returns the *true* FTS5 hit count (before the
/// `MAX_BOOKS_RETURNED` cap) alongside the hydrated rows, in a **single** FTS5
/// pass: the `bm25` MATCH scan runs once inside a `MATERIALIZED` CTE and the
/// total comes from a scalar `(SELECT COUNT(*) FROM matches)` over it. Used by
/// the REST search handler and the RPC search server function so neither has
/// to issue a second `count_search_books` query. Empty/oversized `q` is handled
/// identically to `search_books` and yields `(vec![], 0)`. Issue #241.
pub async fn search_books_with_total(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<(Vec<EbookMetadata>, i64), sqlx::Error> {
    // Cap query length before parsing to bound the FTS5 MATCH expression size,
    // matching `search_palette` (issue #189). Normal/short queries are
    // unaffected; see `cap_query_len`.
    let capped = cap_query_len(q);
    let Some(match_expr) = build_fts_match(&capped) else {
        return Ok((Vec::new(), 0));
    };

    let rows = sqlx::query(
        r#"
        WITH matches AS MATERIALIZED (
            -- Run the FTS5 MATCH + bm25 scan exactly once. bm25() is only
            -- valid in a query that directly references books_fts, so it lives
            -- here; the outer query then both counts and hydrates off the
            -- materialized result without a second FTS5 pass (issue #241).
            SELECT books_fts.rowid AS bid,
                   bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0) AS rank
            FROM books_fts
            JOIN books b ON b.id = books_fts.rowid
            JOIN libraries l ON l.id = b.library_id
            WHERE books_fts MATCH ? AND l.path = ?
        )
        SELECT b.id, b.uuid,
               b.title, b.description, b.series_index, b.has_cover,
               b.pubdate, b.last_modified, b.timestamp, b.isbn, b.accent_color,

               (SELECT COUNT(*) FROM matches)               AS total_count,

               (SELECT bf.filename FROM book_files bf
                 WHERE bf.book_id = b.id
                 ORDER BY (bf.format != 'EPUB'), bf.format
                 LIMIT 1)                                   AS primary_filename,

               (SELECT bf.format FROM book_files bf
                 WHERE bf.book_id = b.id
                 ORDER BY (bf.format != 'EPUB'), bf.format
                 LIMIT 1)                                   AS primary_format,

               (SELECT pub.name FROM books_publishers_link bpl
                  JOIN publishers pub ON pub.id = bpl.publisher
                 WHERE bpl.book = b.id ORDER BY pub.name LIMIT 1)
                                                            AS publisher_name,

               (SELECT lang.code FROM books_languages_link bll
                  JOIN languages lang ON lang.id = bll.language
                 WHERE bll.book = b.id ORDER BY lang.code LIMIT 1)
                                                            AS language_code,

               (SELECT s.name FROM books_series_link bsl
                  JOIN series s ON s.id = bsl.series
                 WHERE bsl.book = b.id ORDER BY s.name LIMIT 1)
                                                            AS series_name,

               (SELECT s.id FROM books_series_link bsl
                  JOIN series s ON s.id = bsl.series
                 WHERE bsl.book = b.id ORDER BY s.name LIMIT 1)
                                                            AS series_link_id,

               (SELECT json_group_array(json_object('id', a_id, 'name', name, 'sort', sort))
                  FROM (SELECT a.id AS a_id, a.name AS name, a.sort AS sort
                          FROM books_authors_link bal
                          JOIN authors a ON a.id = bal.author
                         WHERE bal.book = b.id
                         ORDER BY bal.position))            AS creators_json,

               (SELECT json_group_array(name)
                  FROM (SELECT t.name AS name FROM books_tags_link btl
                          JOIN tags t ON t.id = btl.tag
                         WHERE btl.book = b.id
                         ORDER BY t.name))                  AS subjects_json,

               (SELECT json_group_array(json_object('scheme', scheme, 'value', value))
                  FROM (SELECT scheme, value FROM book_identifiers
                         WHERE book_id = b.id
                         ORDER BY scheme, value))           AS identifiers_json,

               (SELECT json_group_array(format)
                  FROM (SELECT format FROM book_files
                         WHERE book_id = b.id
                         ORDER BY format))                  AS formats_json
        FROM matches m
        JOIN books b ON b.id = m.bid
        ORDER BY m.rank, b.sort, b.id
        LIMIT ?
        "#,
    )
    .bind(&match_expr)
    .bind(library_path)
    .bind(MAX_BOOKS_RETURNED)
    .fetch_all(pool)
    .await?;

    // `total_count` is the scalar `COUNT(*)` over the materialized matches, so
    // it's identical on every row; read it off the first. An empty result set
    // means zero matches.
    let total: i64 = rows.first().map(|r| r.get("total_count")).unwrap_or(0);

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.get("id");
        let has_cover: i64 = r.get("has_cover");
        let primary_filename: Option<String> = r.get("primary_filename");
        let primary_format: Option<String> = r.get("primary_format");
        let filename = match (primary_filename, primary_format) {
            (Some(stem), Some(fmt)) => format!("{stem}.{}", fmt.to_ascii_lowercase()),
            _ => String::new(),
        };
        let series_index: Option<f64> = r.get("series_index");

        let creators: Vec<Contributor> = parse_json_array::<CreatorRow>(r.get("creators_json"))?
            .into_iter()
            .map(|c| Contributor {
                name: c.name,
                role: None,
                file_as: c.sort.filter(|s| !s.is_empty()),
                id: c.id,
            })
            .collect();
        let subjects: Vec<String> = parse_json_array(r.get("subjects_json"))?;
        let identifiers: Vec<Identifier> =
            parse_json_array::<IdentifierRow>(r.get("identifiers_json"))?
                .into_iter()
                .map(|i| Identifier {
                    value: i.value,
                    scheme: Some(i.scheme),
                })
                .collect();

        let uuid: String = r.get("uuid");
        out.push(EbookMetadata {
            id,
            filename,
            title: r.get("title"),
            description: sanitize_description(r.get("description")),
            publisher: r.get("publisher_name"),
            published: r.get("pubdate"),
            modified: r.get("last_modified"),
            language: r.get("language_code"),
            rights: None,
            source: None,
            coverage: None,
            dc_type: None,
            dc_format: None,
            relation: None,
            creators,
            contributors: vec![],
            subjects,
            identifiers,
            series: r.get("series_name"),
            series_index: series_index.map(format_series_index),
            series_id: r.get("series_link_id"),
            epub_version: None,
            unique_identifier: Some(uuid.clone()),
            resource_count: 0,
            spine_count: 0,
            toc_count: 0,
            cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
            accent: r.get("accent_color"),
            formats: parse_json_array(r.get("formats_json"))?,
            added_at: r.get("timestamp"),
            error: None,
            has_override: false,
        });
    }

    // F5.1: bulk-merge metadata overrides.
    let uuids: Vec<String> = out
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for book in &mut out {
        // Snapshot uuid into an owned local so the borrow-check sees the
        // overrides_map lookup as independent of the &mut book passed into
        // apply_overrides below.
        let uuid_owned = book.unique_identifier.clone();
        if let Some(uuid) = uuid_owned.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, uuid, ov, *has_cover_ov);
            }
        }
    }
    backfill_creator_ids(pool, &mut out).await?;

    Ok((out, total))
}

/// Total number of FTS5 hits for `q` under `library_path` (before the
/// `MAX_BOOKS_RETURNED` cap is applied). Empty/whitespace `q` returns 0
/// to mirror `search_books`. Issue #81.
pub async fn count_search_books(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<i64, sqlx::Error> {
    // Cap query length before parsing to mirror `search_books` /
    // `search_palette` (issue #189). Normal/short queries are unaffected;
    // see `cap_query_len`.
    let capped = cap_query_len(q);
    let Some(match_expr) = build_fts_match(&capped) else {
        return Ok(0);
    };
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
          FROM books_fts
          JOIN books b ON b.id = books_fts.rowid
          JOIN libraries l ON l.id = b.library_id
         WHERE books_fts MATCH ? AND l.path = ?
        "#,
    )
    .bind(&match_expr)
    .bind(library_path)
    .fetch_one(pool)
    .await
}
