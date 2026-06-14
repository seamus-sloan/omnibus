//! Library-scoped list/count read paths plus the small `IndexedRow`
//! projection used by the incremental reindex diff.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{EbookLibrary, EbookMetadata};

use super::projection::{
    backfill_creator_ids, merge_overrides_into_books, row_to_ebook, BOOK_COLUMNS,
    MAX_BOOKS_RETURNED,
};

/// Return every book indexed under `library_path`. Thin wrapper around
/// [`list_books_for_paths`] kept for callers that only consult one library
/// (ebook-only reindex paths, override tests).
pub async fn list_books(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<EbookMetadata>, super::BooksError> {
    list_books_for_paths(pool, &[library_path]).await
}

/// Return every book indexed under any of `library_paths`. One round-trip
/// to SQLite: every multi-valued relation is pulled in a single statement
/// using scalar subqueries (for single-valued joins) and `json_group_array`
/// over ordered inner selects (for multi-valued lists), matching the
/// pattern in `get_book`.
///
/// Empty `library_paths` returns an empty vec. The library filter uses
/// `l.path IN (?, …)` so the unified landing path (ebook + audiobook)
/// stays one query instead of two — `book_files.format` joins through
/// unchanged, so per-format facet counts on the landing page still work.
pub async fn list_books_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<Vec<EbookMetadata>, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let rows = fetch_list_rows(pool, library_paths).await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(row_to_ebook(r)?);
    }

    merge_overrides_into_books(pool, &mut out).await?;
    backfill_creator_ids(pool, &mut out).await?;

    Ok(out)
}

/// Build the `SELECT … FROM books … WHERE l.path IN (...)` statement and
/// run it with one bind per path plus the trailing `LIMIT`. Inlines an
/// `?, ?, …` placeholder list because `library_paths` is owned by the
/// caller (at most two entries — ebook + audiobook), so there's no
/// injection surface and a temp table would be heavier than the bind loop.
async fn fetch_list_rows(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, sqlx::Error> {
    let placeholders = std::iter::repeat_n("?", library_paths.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT {BOOK_COLUMNS}
        FROM books b
        JOIN scan_roots l ON l.id = b.library_id
        WHERE l.path IN ({placeholders})
        ORDER BY b.sort, b.id
        LIMIT ?
        "#
    );
    let mut q = sqlx::query(&sql);
    for path in library_paths {
        q = q.bind(*path);
    }
    q = q.bind(MAX_BOOKS_RETURNED);
    q.fetch_all(pool).await
}

/// One row per book under `library_path`, carrying just the bits the
/// incremental reindex diff needs to classify a filesystem stat against
/// the existing index.
///
/// `mtime_epoch` / `size_bytes` come from the matching `book_files` row.
/// Today the scanner only writes `.epub` files, so there's one
/// `book_files` row per book; the `MAX(...)` aggregation is defensive in
/// case a future audiobook scanner adds a sibling format row for the
/// same `books.id`. The diff treats `(mtime_epoch=0, size_bytes=0)` as a
/// "never observed" sentinel (the migration default) and routes those
/// rows through the Backfill branch — no OPF re-parse on first run.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedRow {
    pub uuid: String,
    pub mtime_epoch: i64,
    pub size_bytes: i64,
}

/// Read every indexed book under `library_path`, projecting just the
/// columns the incremental diff needs. Single query; the diff itself is
/// pure CPU on the returned `Vec`.
pub async fn list_indexed_rows(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<IndexedRow>, super::BooksError> {
    let rows = sqlx::query(
        r#"
        SELECT b.uuid                                  AS uuid,
               COALESCE(MAX(bf.mtime_epoch), 0)        AS mtime_epoch,
               COALESCE(MAX(bf.size_bytes), 0)         AS size_bytes
          FROM books b
          JOIN scan_roots l   ON l.id = b.library_id
          LEFT JOIN book_files bf ON bf.book_id = b.id
         WHERE l.path = ?
         GROUP BY b.id, b.uuid
        "#,
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| IndexedRow {
            uuid: r.get("uuid"),
            mtime_epoch: r.get("mtime_epoch"),
            size_bytes: r.get("size_bytes"),
        })
        .collect())
}

/// Format-scoped variant of [`list_indexed_rows`]: returns only books
/// that have **at least one** `book_files` row whose `format` is in
/// `formats` (matched case-insensitively via the `COLLATE NOCASE`
/// `book_files.format` column).
///
/// Used by the per-format reindex paths (ebook / audiobook) so that
/// when the user configures the ebook and audiobook libraries to the
/// same on-disk directory, an audiobook reindex does not classify the
/// EPUB rows as Removed (and vice versa). With this filter the diff
/// only sees the rows the current scan can legitimately account for.
///
/// `formats` is an allow-list of uppercase `book_files.format` values
/// — see [`crate::ebook::EBOOK_FORMATS`] and
/// [`crate::audiobook::AUDIOBOOK_FORMATS`]. An empty slice returns an
/// empty vec (no formats to match against).
pub async fn list_indexed_rows_for_formats(
    pool: &SqlitePool,
    library_path: &str,
    formats: &[&str],
) -> Result<Vec<IndexedRow>, super::BooksError> {
    if formats.is_empty() {
        return Ok(Vec::new());
    }
    // Inline placeholder list — formats is a small static slice owned by
    // the caller, so there's no injection surface here and we avoid the
    // overhead of a temp table or sqlx `Arguments` round-trip.
    let placeholders = std::iter::repeat_n("?", formats.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT b.uuid                                  AS uuid,
               COALESCE(MAX(bf.mtime_epoch), 0)        AS mtime_epoch,
               COALESCE(MAX(bf.size_bytes), 0)         AS size_bytes
          FROM books b
          JOIN scan_roots l   ON l.id = b.library_id
          JOIN book_files bf ON bf.book_id = b.id
         WHERE l.path = ?
           AND bf.format IN ({placeholders})
         GROUP BY b.id, b.uuid
        "#
    );
    let mut q = sqlx::query(&sql).bind(library_path);
    for fmt in formats {
        q = q.bind(*fmt);
    }
    let rows = q.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|r| IndexedRow {
            uuid: r.get("uuid"),
            mtime_epoch: r.get("mtime_epoch"),
            size_bytes: r.get("size_bytes"),
        })
        .collect())
}

/// Diff input for files that were attached to a book in another library
/// or format via `merged_uuids`: one [`IndexedRow`] per merged uuid
/// under `library_path` whose backing `book_files` row (matched by
/// `(book_id, format)`) still exists, carrying that row's stat.
///
/// Scoped by `merged_uuids.library_path` — the *file's* scanned root —
/// rather than the target book's library, because the book an attached
/// file hangs off can live in a different library (Dracula.m4b from the
/// audiobook root attached to the ebook root's Dracula). The INNER JOIN
/// is deliberate — a merged uuid whose attachment row vanished simply
/// isn't returned, so the on-disk file classifies as New and
/// re-attaches.
pub async fn list_merged_rows_for_formats(
    pool: &SqlitePool,
    library_path: &str,
    formats: &[&str],
) -> Result<Vec<IndexedRow>, super::BooksError> {
    if formats.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", formats.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT mu.uuid       AS uuid,
               bf.mtime_epoch AS mtime_epoch,
               bf.size_bytes  AS size_bytes
          FROM merged_uuids mu
          JOIN book_files bf ON bf.book_id = mu.book_id AND bf.format = mu.format
         WHERE mu.library_path = ?
           AND mu.format IN ({placeholders})
        "#
    );
    let mut q = sqlx::query(&sql).bind(library_path);
    for fmt in formats {
        q = q.bind(*fmt);
    }
    let rows = q.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|r| IndexedRow {
            uuid: r.get("uuid"),
            mtime_epoch: r.get("mtime_epoch"),
            size_bytes: r.get("size_bytes"),
        })
        .collect())
}

/// Total number of books currently indexed under `library_path`. Thin
/// wrapper around [`count_books_for_paths`] for single-library callers.
pub async fn count_books(pool: &SqlitePool, library_path: &str) -> Result<i64, super::BooksError> {
    count_books_for_paths(pool, &[library_path]).await
}

/// Total number of books currently indexed under any of `library_paths`.
///
/// Companion to `list_books_for_paths`: `list_books_for_paths` caps the
/// returned vec at `MAX_BOOKS_RETURNED`, so callers that need to surface a
/// truncation hint (UI banner, `X-Total-Count` header) ask the count
/// separately. Single scalar query — cheaper than re-running the full
/// SELECT just to count rows.
pub async fn count_books_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<i64, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", library_paths.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT COUNT(*)
          FROM books b
          JOIN scan_roots l ON l.id = b.library_id
         WHERE l.path IN ({placeholders})
        "#
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for path in library_paths {
        q = q.bind(*path);
    }
    Ok(q.fetch_one(pool).await?)
}

/// Build an `EbookLibrary` from whatever is currently in the DB for
/// `library_path`. Returns an empty library (no error, no books) if the path
/// is `None`.
///
/// The returned `books` vec is capped at `MAX_BOOKS_RETURNED`; callers that
/// need to surface a truncation hint should use
/// [`library_from_db_with_total`] instead. This entrypoint deliberately
/// avoids the extra `count_books` round-trip — non-REST callers (the RPC
/// path, internal lookups) don't need the total and shouldn't pay for it.
pub async fn library_from_db(
    pool: &SqlitePool,
    library_path: Option<&str>,
) -> Result<EbookLibrary, super::BooksError> {
    library_from_db_combined(pool, library_path, None).await
}

/// Same as `library_from_db` but also returns the *true* book count under
/// `library_path` (before the `MAX_BOOKS_RETURNED` cap). Used by the REST
/// handler to set `X-Total-Count` and `X-Total-Cap` response headers so
/// the client can detect a truncated response.
pub async fn library_from_db_with_total(
    pool: &SqlitePool,
    library_path: Option<&str>,
) -> Result<(EbookLibrary, i64), super::BooksError> {
    library_from_db_with_total_combined(pool, library_path, None).await
}

/// Build an `EbookLibrary` spanning the ebook and audiobook libraries
/// together — both rows live in the same `books` table under different
/// `library_id`s, so the unified landing grid is one query over the union.
/// Either path may be `None` (no library configured for that format).
///
/// `EbookLibrary.path` reports the ebook path when set, otherwise the
/// audiobook path; the landing page uses it to key per-library
/// `view_prefs` and to render the subtitle, and treating ebooks as the
/// "primary" key preserves prefs across an audiobook-path edit.
pub async fn library_from_db_combined(
    pool: &SqlitePool,
    ebook_path: Option<&str>,
    audiobook_path: Option<&str>,
) -> Result<EbookLibrary, super::BooksError> {
    let paths = collect_paths(ebook_path, audiobook_path);
    if paths.is_empty() {
        return Ok(EbookLibrary::default());
    }
    let books = list_books_for_paths(pool, &paths).await?;
    Ok(EbookLibrary {
        path: Some(
            ebook_path
                .or(audiobook_path)
                .unwrap_or_default()
                .to_string(),
        ),
        books,
        error: None,
        total: None,
    })
}

/// `library_from_db_combined` + the true total under the union (before the
/// `MAX_BOOKS_RETURNED` cap), for the REST handler's `X-Total-Count` /
/// `X-Total-Cap` headers.
pub async fn library_from_db_with_total_combined(
    pool: &SqlitePool,
    ebook_path: Option<&str>,
    audiobook_path: Option<&str>,
) -> Result<(EbookLibrary, i64), super::BooksError> {
    let paths = collect_paths(ebook_path, audiobook_path);
    if paths.is_empty() {
        return Ok((EbookLibrary::default(), 0));
    }
    let books = list_books_for_paths(pool, &paths).await?;
    let total = count_books_for_paths(pool, &paths).await?;
    Ok((
        EbookLibrary {
            path: Some(
                ebook_path
                    .or(audiobook_path)
                    .unwrap_or_default()
                    .to_string(),
            ),
            books,
            error: None,
            total: None,
        },
        total,
    ))
}

/// Merge ebook + audiobook library paths into a de-duplicated list.
pub fn collect_paths<'a>(ebook: Option<&'a str>, audiobook: Option<&'a str>) -> Vec<&'a str> {
    // De-dup when the user points both at the same on-disk root — the
    // `IN` filter would still return one row per book, but the input
    // shape stays consistent with the single-library calls.
    let mut paths: Vec<&str> = Vec::with_capacity(2);
    if let Some(p) = ebook {
        paths.push(p);
    }
    if let Some(p) = audiobook {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}
