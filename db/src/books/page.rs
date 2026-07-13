//! Keyset-paginated, server-side-sorted, server-side-filtered landing read
//! path. [`list_books_page`] returns one page of the configured
//! library ordered by any of the five [`SortKey`] axes, filtered by the
//! sidebar [`ViewFilters`], positioned by an opaque [`PageCursor`].
//!
//! The cursor encodes the last row's `(sort-value, series-index, id)` for the
//! active axis; the keyset predicate seeks strictly past it under the same
//! `ORDER BY` the index provides, so paging never re-scans earlier rows and
//! never uses `OFFSET`. NULL sort values follow SQLite's natural ordering
//! (first under ASC, last under DESC) so each axis stays a pure index range
//! scan over the `(library_id, <col>, …, id)` composites from migration 0028.
//!
//! Like every read path here the books are decoded via `BOOK_COLUMNS` /
//! `row_to_ebook`, then `metadata_overrides` are merged and creator ids
//! backfilled — but only over the page, not the whole library.

use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sqlx::{Row, SqlitePool};

use omnibus_shared::{EbookMetadata, SortDir, SortKey, ViewFilters};

use super::projection::{
    backfill_creator_ids, merge_overrides_into_books, row_to_ebook, BOOK_COLUMNS,
    MAX_BOOKS_RETURNED,
};

/// Opaque keyset position: the active axis's sort value (`sort`), the
/// secondary numeric key for the Series axis (`idx`, the in-series index;
/// `None` for every other axis), and the row `id` tiebreak. Carried to the
/// client as a base64url(JSON) string and handed back verbatim to fetch the
/// next page. Non-authoritative — a tampered or stale cursor only
/// mis-positions the reader's own page, so it is unsigned.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PageCursor {
    #[serde(rename = "s")]
    pub sort: Option<String>,
    #[serde(rename = "x")]
    pub idx: Option<f64>,
    #[serde(rename = "id")]
    pub id: i64,
}

/// Failure decoding a client-supplied cursor. Coarse on purpose — the server
/// layer maps it to a 400; the body distinction (bad base64 vs. bad JSON)
/// is never actionable to the caller.
#[derive(Debug, thiserror::Error)]
pub enum CursorError {
    #[error("malformed pagination cursor")]
    Malformed,
}

impl PageCursor {
    /// base64url(no-pad) of the compact JSON form. Greppable in logs and
    /// trivially decodable — fine for a single-tenant self-hosted app.
    ///
    /// A non-finite `idx` (a NaN/Inf `series_index` from a corrupt source) is
    /// coerced to `None` before serialization: `serde_json` refuses to encode
    /// non-finite floats, and silently emitting an empty cursor would break
    /// the next page request. `parse_series_index` already blocks such values
    /// at index time; this is belt-and-suspenders for any legacy row.
    pub fn encode(&self) -> String {
        let safe = PageCursor {
            sort: self.sort.clone(),
            idx: self.idx.filter(|n| n.is_finite()),
            id: self.id,
        };
        let json = serde_json::to_vec(&safe).unwrap_or_default();
        URL_SAFE_NO_PAD.encode(json)
    }

    /// Inverse of [`encode`](Self::encode). Any decode failure (bad base64,
    /// bad JSON, wrong shape) collapses to [`CursorError::Malformed`].
    pub fn decode(s: &str) -> Result<Self, CursorError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| CursorError::Malformed)?;
        serde_json::from_slice(&bytes).map_err(|_| CursorError::Malformed)
    }
}

/// One page of the library plus the cursor to fetch the next one. `next` is
/// `None` at end of stream (no further rows exist past this page).
#[derive(Debug, Clone, PartialEq)]
pub struct BookPage {
    pub books: Vec<EbookMetadata>,
    pub next: Option<PageCursor>,
}

/// A value to bind, carried positionally so the keyset / filter / path binds
/// stay in lockstep with the `?` placeholders they generate.
enum SqlVal {
    Text(String),
    Real(f64),
    Int(i64),
}

/// One ordered keyset term: the column SQL plus the cursor's value for it.
/// The trailing term is always the non-null `id`; the earlier (sort) terms
/// are nullable.
enum KeyVal {
    Text(Option<String>),
    Real(Option<f64>),
    Int(i64),
}

/// Return one page of `library_paths` ordered by `sort`/`dir`, filtered by
/// `filters`, starting strictly after `cursor` (or at the top when `None`).
///
/// `limit` is clamped to `[1, MAX_BOOKS_RETURNED]`. Fetches one extra row to
/// decide whether a further page exists: a full `limit` page yields a
/// `next` cursor built from the last returned row; a short page yields
/// `next == None`. Empty `library_paths` returns an empty page.
pub async fn list_books_page(
    pool: &SqlitePool,
    library_paths: &[&str],
    sort: SortKey,
    dir: SortDir,
    filters: &ViewFilters,
    cursor: Option<&PageCursor>,
    limit: i64,
) -> Result<BookPage, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(BookPage {
            books: Vec::new(),
            next: None,
        });
    }
    let limit = limit.clamp(1, MAX_BOOKS_RETURNED);

    let (sql, mut binds) = build_page_sql(sort, dir, library_paths, filters, cursor);
    // Fetch one beyond the page so a full page can advertise a `next` cursor
    // without a trailing empty round-trip.
    binds.push(SqlVal::Int(limit + 1));

    let mut q = sqlx::query(&sql);
    for v in &binds {
        q = match v {
            SqlVal::Text(s) => q.bind(s.as_str()),
            SqlVal::Real(r) => q.bind(*r),
            SqlVal::Int(i) => q.bind(*i),
        };
    }
    let rows = q.fetch_all(pool).await?;

    let take = rows.len().min(limit as usize);
    let next = (rows.len() as i64 > limit).then(|| cursor_from_row(&rows[take - 1]));

    let mut books = Vec::with_capacity(take);
    for r in &rows[..take] {
        books.push(row_to_ebook(r)?);
    }
    merge_overrides_into_books(pool, &mut books).await?;
    backfill_creator_ids(pool, &mut books).await?;

    Ok(BookPage { books, next })
}

/// Build the paginated `SELECT` and its positional binds for `sort`/`dir`
/// over `library_paths`, filtered by `filters` and seeked past `cursor`.
/// Split out of [`list_books_page`] so the SQL-construction stage is
/// independently testable from the fetch/decode stage.
fn build_page_sql(
    sort: SortKey,
    dir: SortDir,
    library_paths: &[&str],
    filters: &ViewFilters,
    cursor: Option<&PageCursor>,
) -> (String, Vec<SqlVal>) {
    let (primary, secondary) = axis_sort_columns(sort);
    let dir_sql = match dir {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    };

    let mut binds: Vec<SqlVal> = Vec::new();
    let path_ph = placeholders(library_paths.len());
    for p in library_paths {
        binds.push(SqlVal::Text((*p).to_string()));
    }
    let filter_sql = filter_predicates(filters, &mut binds);
    let keyset_sql = match cursor {
        Some(c) => format!(
            " AND {}",
            keyset_predicate(&axis_terms(sort, c), dir, &mut binds)
        ),
        None => String::new(),
    };

    let cursor_idx_sql = secondary.unwrap_or("NULL");
    let order_by = match secondary {
        Some(sec) => format!("{primary} {dir_sql}, {sec} {dir_sql}, b.id {dir_sql}"),
        None => format!("{primary} {dir_sql}, b.id {dir_sql}"),
    };

    let sql = format!(
        r"
        SELECT {BOOK_COLUMNS},
               {primary} AS cursor_sort,
               {cursor_idx_sql} AS cursor_idx
          FROM books b
          JOIN scan_roots l ON l.id = b.library_id
         WHERE l.path IN ({path_ph})
           AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
           {filter_sql}{keyset_sql}
         ORDER BY {order_by}
         LIMIT ?
        "
    );
    (sql, binds)
}

/// The `(primary, secondary)` `books` sort columns for each axis. `primary`
/// always yields `TEXT`; `secondary` (the in-series index) is `Some` only for
/// [`SortKey::Series`].
///
/// The two time axes read INTEGER unix-seconds (migration 0038) but format
/// them to fixed-width ISO here, for the same reason the projection does: the
/// cursor round-trips the primary value as `Option<String>` and compares it
/// lexicographically, which stays chronologically correct on ISO text. (The
/// trade-off is that ordering by the expression can't use the raw-column
/// keyset index — acceptable at a self-hosted library's scale.)
fn axis_sort_columns(sort: SortKey) -> (&'static str, Option<&'static str>) {
    match sort {
        SortKey::Title => ("b.sort", None),
        SortKey::Author => ("b.author_sort", None),
        SortKey::LastUpdated => (
            "strftime('%Y-%m-%dT%H:%M:%SZ', b.last_modified, 'unixepoch')",
            None,
        ),
        SortKey::NewestAdded => (
            "strftime('%Y-%m-%dT%H:%M:%SZ', b.timestamp, 'unixepoch')",
            None,
        ),
        SortKey::Series => ("b.series_sort", Some("b.series_index")),
    }
}

/// Build the ordered keyset terms for `sort` against `cursor`: the primary
/// sort value, the secondary (series-index) value for the Series axis, then
/// the `id` tiebreak.
fn axis_terms(sort: SortKey, cursor: &PageCursor) -> Vec<(String, KeyVal)> {
    let (primary, secondary) = axis_sort_columns(sort);
    let mut terms = vec![(primary.to_string(), KeyVal::Text(cursor.sort.clone()))];
    if let Some(sec) = secondary {
        terms.push((sec.to_string(), KeyVal::Real(cursor.idx)));
    }
    terms.push(("b.id".to_string(), KeyVal::Int(cursor.id)));
    terms
}

/// Read the `(cursor_sort, cursor_idx, id)` aliases off a boundary row into
/// the next-page cursor.
fn cursor_from_row(row: &sqlx::sqlite::SqliteRow) -> PageCursor {
    PageCursor {
        sort: row.get::<Option<String>, _>("cursor_sort"),
        idx: row.get::<Option<f64>, _>("cursor_idx"),
        id: row.get::<i64, _>("id"),
    }
}

/// `?, ?, …` of length `n`.
pub(super) fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(", ")
}

/// Build the `( clause OR clause … )` keyset predicate selecting rows strictly
/// after `terms` in the `dir` ordering. Each clause fixes the higher-priority
/// terms to equality (`IS`-style, NULL-safe) and applies a strict, NULL-aware
/// comparison on the next term — the standard row-value lexicographic
/// expansion. Binds are pushed in exact textual order.
fn keyset_predicate(terms: &[(String, KeyVal)], dir: SortDir, binds: &mut Vec<SqlVal>) -> String {
    let mut clauses: Vec<String> = Vec::new();
    for i in 0..terms.len() {
        let mut parts: Vec<String> = Vec::new();
        let mut local: Vec<SqlVal> = Vec::new();
        for (col, val) in &terms[..i] {
            parts.push(eq_sql(col, val, &mut local));
        }
        // A statically-false strict term (DESC past a NULL — nothing sorts
        // after a NULL when NULLs are last) drops the whole clause, including
        // the equality binds we tentatively built for it.
        if let Some(s) = strict_after_sql(&terms[i].0, &terms[i].1, dir, &mut local) {
            parts.push(s);
            clauses.push(format!("({})", parts.join(" AND ")));
            binds.append(&mut local);
        }
    }
    format!("({})", clauses.join(" OR "))
}

/// NULL-safe equality `col IS <val>` for a fixed higher-priority term.
fn eq_sql(col: &str, val: &KeyVal, binds: &mut Vec<SqlVal>) -> String {
    match val {
        KeyVal::Text(None) | KeyVal::Real(None) => format!("{col} IS NULL"),
        KeyVal::Text(Some(s)) => {
            binds.push(SqlVal::Text(s.clone()));
            format!("{col} = ?")
        }
        KeyVal::Real(Some(r)) => {
            binds.push(SqlVal::Real(*r));
            format!("{col} = ?")
        }
        KeyVal::Int(i) => {
            binds.push(SqlVal::Int(*i));
            format!("{col} = ?")
        }
    }
}

/// Strict "after `val`" comparison on `col` honoring `dir` and SQLite's
/// natural NULL ordering (NULLs first under ASC, last under DESC). Returns
/// `None` when the comparison is statically empty (DESC past a NULL), so the
/// caller drops that clause.
fn strict_after_sql(
    col: &str,
    val: &KeyVal,
    dir: SortDir,
    binds: &mut Vec<SqlVal>,
) -> Option<String> {
    let asc = dir == SortDir::Asc;
    match val {
        KeyVal::Int(i) => {
            binds.push(SqlVal::Int(*i));
            Some(format!("{col} {} ?", if asc { ">" } else { "<" }))
        }
        KeyVal::Text(None) | KeyVal::Real(None) => {
            // ASC: every non-NULL row sorts after a NULL one. DESC: nothing
            // does (NULLs are last) — drop the clause.
            asc.then(|| format!("{col} IS NOT NULL"))
        }
        KeyVal::Text(Some(s)) => {
            binds.push(SqlVal::Text(s.clone()));
            Some(strict_present(col, asc))
        }
        KeyVal::Real(Some(r)) => {
            binds.push(SqlVal::Real(*r));
            Some(strict_present(col, asc))
        }
    }
}

/// Strict comparison against a present (non-NULL) cursor value. ASC keeps
/// only larger non-NULL rows; DESC keeps smaller rows plus the NULL tail.
fn strict_present(col: &str, asc: bool) -> String {
    if asc {
        format!("({col} IS NOT NULL AND {col} > ?)")
    } else {
        format!("({col} IS NULL OR {col} < ?)")
    }
}

/// Count the library rows matching `filters` — the filtered-total companion
/// to [`list_books_page`], sharing its predicates. With empty filters this
/// matches `count_books_for_paths`: fileless (ghosted) books are excluded
/// either way.
pub async fn count_books_page(
    pool: &SqlitePool,
    library_paths: &[&str],
    filters: &ViewFilters,
) -> Result<i64, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(0);
    }
    let mut binds: Vec<SqlVal> = library_paths
        .iter()
        .map(|p| SqlVal::Text((*p).to_string()))
        .collect();
    let path_ph = placeholders(library_paths.len());
    let filter_sql = filter_predicates(filters, &mut binds);
    let sql = format!(
        r"
        SELECT COUNT(*)
          FROM books b
          JOIN scan_roots l ON l.id = b.library_id
         WHERE l.path IN ({path_ph})
           AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id){filter_sql}
        "
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for v in &binds {
        q = match v {
            SqlVal::Text(s) => q.bind(s.as_str()),
            SqlVal::Real(r) => q.bind(*r),
            SqlVal::Int(i) => q.bind(*i),
        };
    }
    Ok(q.fetch_one(pool).await?)
}

/// Build the ` AND EXISTS(…)` server-side filter conjuncts. Each non-empty
/// facet group ANDs across groups and ORs within (an `IN (…)` list), matching
/// the former client-side `matches_filters`.
fn filter_predicates(filters: &ViewFilters, binds: &mut Vec<SqlVal>) -> String {
    let mut out = String::new();
    if !filters.authors.is_empty() {
        out.push_str(&exists_in(
            "books_authors_link bal JOIN authors a ON a.id = bal.author",
            "bal.book = b.id",
            "a.name",
            &filters.authors,
            binds,
        ));
    }
    if !filters.series.is_empty() {
        out.push_str(&exists_in(
            "books_series_link bsl JOIN series s ON s.id = bsl.series",
            "bsl.book = b.id",
            "s.name",
            &filters.series,
            binds,
        ));
    }
    if !filters.formats.is_empty() {
        // `book_files.format` is COLLATE NOCASE, so the lowercase chip keys
        // (`"epub"`) match the stored uppercase (`"EPUB"`) without folding.
        out.push_str(&exists_in(
            "book_files bf",
            "bf.book_id = b.id",
            "bf.format",
            &filters.formats,
            binds,
        ));
    }
    if !filters.tags.is_empty() {
        out.push_str(&exists_in(
            "books_tags_link btl JOIN tags t ON t.id = btl.tag",
            "btl.book = b.id",
            "t.name",
            &filters.tags,
            binds,
        ));
    }
    out
}

/// ` AND EXISTS (SELECT 1 FROM <from> WHERE <join> AND <col> IN (?, …))`,
/// pushing each value as a bind.
fn exists_in(
    from: &str,
    join: &str,
    col: &str,
    values: &[String],
    binds: &mut Vec<SqlVal>,
) -> String {
    let ph = placeholders(values.len());
    for v in values {
        binds.push(SqlVal::Text(v.clone()));
    }
    format!(" AND EXISTS (SELECT 1 FROM {from} WHERE {join} AND {col} IN ({ph}))")
}

#[cfg(test)]
mod tests;
