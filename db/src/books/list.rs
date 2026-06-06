//! Library-scoped list/count read paths plus the small `IndexedRow`
//! projection used by the incremental reindex diff.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata, Identifier};

use crate::helpers::format_series_index;
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk};

use super::projection::{
    backfill_creator_ids, parse_json_array, sanitize_description, CreatorRow, IdentifierRow,
    MAX_BOOKS_RETURNED,
};

/// Return every book indexed under `library_path`. Thin wrapper around
/// [`list_books_for_paths`] kept for callers that only consult one library
/// (ebook-only reindex paths, override tests).
pub async fn list_books(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
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
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    // Inline placeholder list: `library_paths` is owned by the caller (at
    // most two entries — ebook + audiobook), so there's no injection
    // surface and a temp table would be heavier than the bind loop below.
    let placeholders = std::iter::repeat_n("?", library_paths.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT b.id, b.uuid,
               b.title, b.description, b.series_index, b.has_cover,
               b.pubdate, b.last_modified, b.timestamp, b.isbn, b.accent_color,

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
        FROM books b
        JOIN libraries l ON l.id = b.library_id
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
    let rows = q.fetch_all(pool).await?;

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
            creators,
            subjects,
            identifiers,
            series: r.get("series_name"),
            series_index: series_index.map(format_series_index),
            series_id: r.get("series_link_id"),
            unique_identifier: Some(uuid.clone()),
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

    Ok(out)
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
) -> Result<Vec<IndexedRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT b.uuid                                  AS uuid,
               COALESCE(MAX(bf.mtime_epoch), 0)        AS mtime_epoch,
               COALESCE(MAX(bf.size_bytes), 0)         AS size_bytes
          FROM books b
          JOIN libraries l   ON l.id = b.library_id
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
) -> Result<Vec<IndexedRow>, sqlx::Error> {
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
          JOIN libraries l   ON l.id = b.library_id
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

/// Total number of books currently indexed under `library_path`. Thin
/// wrapper around [`count_books_for_paths`] for single-library callers.
pub async fn count_books(pool: &SqlitePool, library_path: &str) -> Result<i64, sqlx::Error> {
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
) -> Result<i64, sqlx::Error> {
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
          JOIN libraries l ON l.id = b.library_id
         WHERE l.path IN ({placeholders})
        "#
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for path in library_paths {
        q = q.bind(*path);
    }
    q.fetch_one(pool).await
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
) -> Result<EbookLibrary, sqlx::Error> {
    library_from_db_combined(pool, library_path, None).await
}

/// Same as `library_from_db` but also returns the *true* book count under
/// `library_path` (before the `MAX_BOOKS_RETURNED` cap). Used by the REST
/// handler to set `X-Total-Count` and `X-Total-Cap` response headers so
/// the client can detect a truncated response.
pub async fn library_from_db_with_total(
    pool: &SqlitePool,
    library_path: Option<&str>,
) -> Result<(EbookLibrary, i64), sqlx::Error> {
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
) -> Result<EbookLibrary, sqlx::Error> {
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
) -> Result<(EbookLibrary, i64), sqlx::Error> {
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

fn collect_paths<'a>(ebook: Option<&'a str>, audiobook: Option<&'a str>) -> Vec<&'a str> {
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
