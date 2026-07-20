//! FTS5-backed search read path. Wraps the `books_fts` virtual table with
//! the same scalar-subquery projection the other read paths use, so search
//! results hydrate into the same `EbookMetadata` shape `list_books` /
//! `get_book` return.

use omnibus_shared::EbookMetadata;
use sqlx::{Row, SqlitePool};

use crate::helpers::{build_fts_match, cap_query_len, library_paths_json};

use super::projection::{
    backfill_creator_ids, merge_overrides_into_books, row_to_ebook, BOOK_COLUMNS,
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
) -> Result<Vec<EbookMetadata>, super::BooksError> {
    search_books_for_paths(pool, &[library_path], q).await
}

/// Full-text search across every configured library path.
pub async fn search_books_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
) -> Result<Vec<EbookMetadata>, super::BooksError> {
    let (books, _total) = search_books_for_paths_with_total(pool, library_paths, q).await?;
    Ok(books)
}

/// Same as [`search_books`] but returns the *true* FTS5 hit count (before the
/// `MAX_BOOKS_RETURNED` cap) alongside the hydrated rows, in a **single** FTS5
/// pass: the `bm25` MATCH scan runs once inside a `MATERIALIZED` CTE and the
/// total comes from a scalar `(SELECT COUNT(*) FROM matches)` over it. Used by
/// the REST search handler and the RPC search server function so neither has
/// to issue a second `count_search_books` query. Empty/oversized `q` is handled
/// identically to `search_books` and yields `(vec![], 0)`.
pub async fn search_books_with_total(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<(Vec<EbookMetadata>, i64), super::BooksError> {
    search_books_for_paths_with_total(pool, &[library_path], q).await
}

/// Search across `library_paths` and return capped rows plus the true hit count.
pub async fn search_books_for_paths_with_total(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
) -> Result<(Vec<EbookMetadata>, i64), super::BooksError> {
    if library_paths.is_empty() {
        return Ok((Vec::new(), 0));
    }
    // Cap query length before parsing to bound the FTS5 MATCH expression size,
    // matching `search_palette` (issue #189). Normal/short queries are
    // unaffected; see `cap_query_len`.
    let capped = cap_query_len(q);
    let Some(match_expr) = build_fts_match(&capped) else {
        return Ok((Vec::new(), 0));
    };

    let rows = fetch_search_rows(pool, library_paths, &match_expr).await?;

    // `total_count` is the scalar `COUNT(*)` over the materialized matches, so
    // it's identical on every row; read it off the first. An empty result set
    // means zero matches.
    let total: i64 = rows.first().map(|r| r.get("total_count")).unwrap_or(0);

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(row_to_ebook(r)?);
    }

    merge_overrides_into_books(pool, &mut out).await?;
    backfill_creator_ids(pool, &mut out).await?;

    Ok((out, total))
}

/// Run the single-pass FTS5 MATCH + hydrate query: bm25 scan inside a
/// `MATERIALIZED` CTE, then the outer SELECT joins back to `books` with the
/// shared `BOOK_COLUMNS` projection plus a scalar `(SELECT COUNT(*))`
/// `total_count` column. bm25() is only valid in a query that directly
/// references books_fts, so it must live inside the CTE.
async fn fetch_search_rows(
    pool: &SqlitePool,
    library_paths: &[&str],
    match_expr: &str,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, sqlx::Error> {
    let sql = format!(
        r"
        WITH matches AS MATERIALIZED (
            SELECT books_fts.rowid AS bid,
                   bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0) AS rank
            FROM books_fts
            JOIN books b ON b.id = books_fts.rowid
            JOIN scan_roots l ON l.id = b.library_id
            WHERE books_fts MATCH ?
              AND (l.path IN (SELECT value FROM json_each(?))
                   OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid))
        )
        SELECT {BOOK_COLUMNS},
               (SELECT COUNT(*) FROM matches)               AS total_count
        FROM matches m
        JOIN books b ON b.id = m.bid
        ORDER BY m.rank, b.sort, b.id
        LIMIT ?
        "
    );
    sqlx::query(&sql)
        .bind(match_expr)
        .bind(library_paths_json(library_paths))
        .bind(MAX_BOOKS_RETURNED)
        .fetch_all(pool)
        .await
}

/// Total number of FTS5 hits for `q` under `library_path` (before the
/// `MAX_BOOKS_RETURNED` cap is applied). Empty/whitespace `q` returns 0
/// to mirror `search_books`.
pub async fn count_search_books(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<i64, super::BooksError> {
    count_search_books_for_paths(pool, &[library_path], q).await
}

/// Count FTS5 hits across every configured library path.
pub async fn count_search_books_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
) -> Result<i64, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(0);
    }
    // Cap query length before parsing to mirror `search_books` /
    // `search_palette` (issue #189). Normal/short queries are unaffected;
    // see `cap_query_len`.
    let capped = cap_query_len(q);
    let Some(match_expr) = build_fts_match(&capped) else {
        return Ok(0);
    };
    Ok(sqlx::query_scalar::<_, i64>(
        r"
        SELECT COUNT(*)
          FROM books_fts
          JOIN books b ON b.id = books_fts.rowid
          JOIN scan_roots l ON l.id = b.library_id
         WHERE books_fts MATCH ?
           AND (l.path IN (SELECT value FROM json_each(?))
                OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid))
        ",
    )
    .bind(&match_expr)
    .bind(library_paths_json(library_paths))
    .fetch_one(pool)
    .await?)
}
