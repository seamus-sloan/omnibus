//! Single-book read paths: `get_book` (id), `get_book_by_uuid`, and the
//! `resolve_book_id_by_uuid` helper that the covers/thumbs/mobile routes
//! use to translate a stable uuid to the current id.

use std::collections::HashMap;

use omnibus_shared::EbookMetadata;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::metadata_overrides::{apply_overrides, get_metadata_overrides};

use super::projection::{
    backfill_creator_ids, merge_overrides_into_books, row_to_ebook, BOOK_COLUMNS,
};

/// Fetch a single book by its stable `books.id`. Returns `None` if not found.
///
/// One round-trip to SQLite: the main `books` row plus every m2m relation are
/// pulled in a single statement using scalar subqueries (for single-valued
/// joins) and `json_group_array` over ordered inner selects (for multi-valued
/// lists). Determinism is preserved by always ordering the inner selects —
/// EPUB-preferred for the primary file, alphabetical for publisher/language/
/// series/tags/formats/identifiers, and `books_authors_link.position` for
/// authors.
///
/// Multi-valued lists are returned as JSON via SQLite's `json_group_array` +
/// `json_object`, which round-trips any UTF-8 — including control chars and
/// punctuation that a delimiter-based encoding would corrupt. The Rust side
/// parses each blob with `serde_json`. Empty aggregates come back as `"[]"`,
/// so the `Option<String>` path only fires when the column itself was NULL.
pub async fn get_book(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<EbookMetadata>, super::BooksError> {
    let Some(row) = fetch_book_row(pool, id).await? else {
        return Ok(None);
    };

    let uuid: String = row.try_get("uuid")?;
    let mut book = row_to_ebook(&row)?;

    // F5.1: merge user-supplied metadata overrides, gated by the owning
    // scan root's configured metadata-source precedence (#972).
    if let Some((ov, has_cover_ov)) = get_metadata_overrides(pool, &uuid).await? {
        let precedence =
            crate::settings::metadata_precedence_by_uuid(pool, std::slice::from_ref(&uuid))
                .await?
                .remove(&uuid)
                .unwrap_or_else(|| omnibus_shared::DEFAULT_METADATA_PRECEDENCE.to_vec());
        apply_overrides(&mut book, &uuid, &ov, has_cover_ov, &precedence);
    }

    backfill_series_id_by_name(pool, &mut book).await?;
    // Override Contributors are stored by name only — apply_overrides
    // therefore leaves `id` unset, which renders the breadcrumb /
    // "More by …" author link as an unclickable span even when an
    // `authors` row with that name exists. Backfill the id from the
    // authors table by name. Mirrors the series_id backfill above.
    backfill_creator_ids(pool, std::slice::from_mut(&mut book)).await?;

    let files = get_book_files(pool, id).await?;
    // Size of the EPUB the hero send would deliver (file_id None resolves the
    // lowest-ordinal EPUB, same as `book_file_path`), so the export menu can
    // gate the email button on Kindle's size cap.
    book.epub_size_bytes = files
        .iter()
        .filter(|f| f.format.eq_ignore_ascii_case("EPUB"))
        .min_by_key(|f| f.ordinal)
        .map(|f| f.size_bytes);
    // Always published, including for the single-file-per-format case this
    // used to omit. Each row carries the file's content validator, which is
    // what an offline client compares against its download snapshot — and a
    // typical library is *entirely* single-file books, so withholding the
    // rows there withheld the validator from exactly the common case.
    //
    // Whether to *render* per-file detail or a compact format badge stays a
    // presentation question, decided by the surfaces that show it (see
    // `files_list` in `frontend/src/pages/book_detail/mobile.rs` and the
    // per-format count in `format_switcher`).
    book.book_files = files;

    Ok(Some(book))
}

/// Run the single-row `SELECT … WHERE b.id = ?` against `books` with the
/// shared `BOOK_COLUMNS` projection. Returns `None` if the id is unknown.
async fn fetch_book_row(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> {
    let sql = format!(
        r"
        SELECT {BOOK_COLUMNS}
        FROM books b
        WHERE b.id = ?
        "
    );
    sqlx::query(&sql).bind(id).fetch_optional(pool).await
}

/// `apply_overrides` rewrites `book.series` from the JSON blob but can't
/// touch the relational `books_series_link` row, so a book whose series
/// exists only as an override ends up with the series *name* but no
/// `series_id`. Backfill it by looking up the series by name so the
/// detail-page `Link` to `/series/:id` resolves.
async fn backfill_series_id_by_name(
    pool: &SqlitePool,
    book: &mut EbookMetadata,
) -> Result<(), sqlx::Error> {
    if book.series_id.is_some() {
        return Ok(());
    }
    if let Some(name) = book.series.as_deref().filter(|s| !s.is_empty()) {
        book.series_id = sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    }
    Ok(())
}

/// Look up a book by its stable `books.uuid` and return the same merged
/// metadata `get_book` produces. Delegates to `get_book` after resolving
/// the uuid to an id so the body stays a single source of truth.
///
/// This is the read path for `/books/:uuid` and `/api/ebooks/:uuid` —
/// the URL-stable counterparts to the renumbering `:id` routes.
pub async fn get_book_by_uuid(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<EbookMetadata>, super::BooksError> {
    let Some(id) = resolve_book_id_by_uuid(pool, uuid).await? else {
        return Ok(None);
    };
    get_book(pool, id).await
}

/// Map a `books.uuid` to its current `books.id`. `books.uuid` is
/// `UNIQUE`, so this is one indexed lookup. Returns `None` if the uuid
/// is unknown — handlers translate to a 404.
///
/// Falls back to `merged_uuids`: a uuid that was merged into (or
/// auto-attached to) another book resolves to the surviving book, so
/// old detail-page links, cover/thumb URLs, and in-flight progress
/// POSTs keep working after a merge.
///
/// The covers / thumbs / mobile-ebooks routes use this to keep their
/// URLs uuid-keyed externally while reusing the existing id-keyed
/// internal helpers (`get_cover`, the thumbnail pipeline) unchanged.
pub async fn resolve_book_id_by_uuid(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<i64>, super::BooksError> {
    resolve_book_id_by_uuid_exec(pool, uuid).await
}

/// Executor-generic counterpart to [`resolve_book_id_by_uuid`], single-sourcing
/// the `books`/`merged_uuids` UNION fallback so callers inside an open
/// transaction (e.g. session inserts) resolve book identity identically to the
/// pool-based read paths. Pass `&pool` for a standalone lookup or `&mut *tx`
/// from within a transaction.
pub async fn resolve_book_id_by_uuid_exec<'e, E>(
    executor: E,
    uuid: &str,
) -> Result<Option<i64>, super::BooksError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM books WHERE uuid = ?1
         UNION ALL
         SELECT book_id FROM merged_uuids WHERE uuid = ?1
         LIMIT 1",
    )
    .bind(uuid)
    .fetch_optional(executor)
    .await?)
}

/// Return `books.last_modified` (unix seconds), defaulting to `0` when the
/// column is NULL. Used by the Kobo cover-image handler to derive a weak
/// `ETag` from `(book id, last_modified)`; callers are expected to have
/// already confirmed `id` exists (e.g. via [`resolve_book_id_by_uuid`]).
pub async fn book_last_modified_for(pool: &SqlitePool, id: i64) -> Result<i64, super::BooksError> {
    Ok(sqlx::query_scalar(
        "SELECT CAST(COALESCE(last_modified, 0) AS INTEGER) FROM books WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

/// Bulk counterpart to [`resolve_book_id_by_uuid`]: map each input uuid to
/// its current `books.id`, replicating the `merged_uuids` fallback so a
/// merged/attached uuid still resolves to the surviving book. UUIDs that
/// match neither table are simply absent from the returned map.
///
/// Emits one `SELECT` per chunk, chunked at 499 uuids because each chunk
/// binds its uuids twice (the `books` and `merged_uuids` legs of the
/// `UNION ALL`), keeping the bound-parameter count under SQLite's 999 limit.
pub(crate) async fn resolve_book_ids_bulk(
    pool: &SqlitePool,
    uuids: &[String],
) -> Result<HashMap<String, i64>, super::BooksError> {
    let mut map = HashMap::with_capacity(uuids.len());
    for chunk in uuids.chunks(499) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT uuid, id FROM books WHERE uuid IN ({placeholders})
             UNION ALL
             SELECT uuid, book_id FROM merged_uuids WHERE uuid IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, i64)>(&sql);
        // Bind the chunk twice — once for each `IN (...)` leg, in statement order.
        for u in chunk.iter().chain(chunk.iter()) {
            q = q.bind(u);
        }
        for (uuid, id) in q.fetch_all(pool).await? {
            // A `books` row wins over a `merged_uuids` ledger key on the
            // (practically impossible) collision, mirroring the `books`-first
            // ordering of `resolve_book_id_by_uuid`'s `UNION ALL … LIMIT 1`.
            map.entry(uuid).or_insert(id);
        }
    }
    Ok(map)
}

/// Bulk-fetch merged metadata for a set of `books.id`s, applying overrides
/// and the creator-id backfill exactly as `list_books_for_paths` does.
/// Emits one `SELECT … WHERE b.id IN (…)` per 499-id chunk plus the two
/// bulk override/creator passes. Return order is unspecified; callers pair
/// rows via [`EbookMetadata::id`]. Unknown ids are simply absent.
pub(crate) async fn get_books_by_ids(
    pool: &SqlitePool,
    ids: &[i64],
) -> Result<Vec<EbookMetadata>, super::BooksError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(499) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT {BOOK_COLUMNS} FROM books b WHERE b.id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for r in q.fetch_all(pool).await? {
            out.push(row_to_ebook(&r)?);
        }
    }
    merge_overrides_into_books(pool, &mut out).await?;
    backfill_creator_ids(pool, &mut out).await?;
    Ok(out)
}

/// Resolve any book reference (its own durable `books.uuid` or a
/// `merged_uuids` ledger key for a format-merged/attached file) to the
/// **canonical** `books.uuid` of the surviving book. `None` when the uuid
/// matches neither. Used by the F1 user-data write paths so a row always
/// stores the durable identity (and a merged uuid collapses onto its target)
/// — keeping `(user_id, book_uuid, format)` uniqueness correct.
pub async fn resolve_canonical_book_uuid_exec<'e, E>(
    executor: E,
    uuid: &str,
) -> Result<Option<String>, super::BooksError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT uuid FROM books WHERE uuid = ?1
         UNION ALL
         SELECT b.uuid FROM merged_uuids m JOIN books b ON b.id = m.book_id WHERE m.uuid = ?1
         LIMIT 1",
    )
    .bind(uuid)
    .fetch_optional(executor)
    .await?)
}

/// Pool-based [`resolve_canonical_book_uuid_exec`].
pub async fn resolve_canonical_book_uuid(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<String>, super::BooksError> {
    resolve_canonical_book_uuid_exec(pool, uuid).await
}

/// Bulk counterpart to [`resolve_canonical_book_uuid_exec`]: resolve every
/// distinct uuid in `uuids` to its canonical `books.uuid` in one round-trip
/// (chunked at 499 to stay under SQLite's 999 bind-parameter cap — each uuid
/// is bound twice for the `books`/`merged_uuids` UNION). Returns a map from
/// input uuid to canonical uuid; uuids unknown in **both** tables are absent
/// from the map. Callers that would otherwise loop `resolve_canonical_book_uuid_exec`
/// (e.g. the batched `post_sessions` write path) use this to collapse an N-report
/// batch's 2N queries into `chunks + N` inserts.
pub async fn resolve_canonical_book_uuids_bulk_exec(
    tx: &mut Transaction<'_, Sqlite>,
    uuids: &[String],
) -> Result<HashMap<String, String>, super::BooksError> {
    let mut out: HashMap<String, String> = HashMap::new();
    if uuids.is_empty() {
        return Ok(out);
    }
    // Dedup input to shrink chunks and skip repeat work when the same book
    // appears in multiple reports.
    let mut unique: Vec<&str> = uuids.iter().map(String::as_str).collect();
    unique.sort_unstable();
    unique.dedup();
    // 499 uuids × 2 binds/uuid = 998 params, under SQLite's 999 cap. Mirrors
    // the chunk pattern in `db/src/sync/audiobooks.rs`.
    for chunk in unique.chunks(499) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT uuid AS input, uuid AS canonical
               FROM books
              WHERE uuid IN ({placeholders})
             UNION ALL
             SELECT m.uuid AS input, b.uuid AS canonical
               FROM merged_uuids m
               JOIN books b ON b.id = m.book_id
              WHERE m.uuid IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, String)>(&sql);
        // Bind twice — once for each `IN (...)` in the UNION.
        for u in chunk {
            q = q.bind(*u);
        }
        for u in chunk {
            q = q.bind(*u);
        }
        for (input, canonical) in q.fetch_all(&mut **tx).await? {
            // `books` wins over `merged_uuids` when both branches somehow
            // return the same input (they shouldn't — a uuid is either live
            // or merged, never both — but keep the direct row's identity if
            // this invariant is ever loosened).
            out.entry(input).or_insert(canonical);
        }
    }
    Ok(out)
}

/// Bulk counterpart to [`book_file_path`]: resolve every id in `ids` to its
/// on-disk path for `format` in one round trip. Chunked at 499 ids to stay
/// under SQLite's bind-parameter cap; when multiple files share a format,
/// the lowest `ordinal` wins per id, same as `book_file_path`. Ids with no
/// matching file are absent from the map.
pub async fn book_file_paths(
    pool: &SqlitePool,
    ids: &[i64],
    format: &str,
) -> Result<HashMap<i64, std::path::PathBuf>, super::BooksError> {
    let mut map = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(499) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT b.id, COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                    bf.filename, bf.format \
             FROM books b \
             JOIN scan_roots l ON l.id = b.library_id \
             JOIN book_files bf ON bf.book_id = b.id \
             WHERE b.id IN ({placeholders}) AND bf.format = ? COLLATE NOCASE \
             ORDER BY b.id, bf.ordinal"
        );
        let mut q = sqlx::query_as::<_, (i64, String, String, String, String)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        q = q.bind(format);
        for (id, lib, dir, stem, fmt) in q.fetch_all(pool).await? {
            map.entry(id).or_insert_with(|| {
                std::path::Path::new(&lib)
                    .join(&dir)
                    .join(format!("{stem}.{}", fmt.to_lowercase()))
            });
        }
    }
    Ok(map)
}

/// Resolve the on-disk path of a book's file for the given format
/// (e.g. "EPUB"). When multiple files of the same format exist, returns
/// the one with the lowest ordinal. Ok(None) when absent.
pub async fn book_file_path(
    pool: &SqlitePool,
    id: i64,
    format: &str,
) -> Result<Option<std::path::PathBuf>, super::BooksError> {
    let row = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format \
         FROM books b \
         JOIN scan_roots l ON l.id = b.library_id \
         JOIN book_files bf ON bf.book_id = b.id \
         WHERE b.id = ? AND bf.format = ? COLLATE NOCASE \
         ORDER BY bf.ordinal LIMIT 1",
    )
    .bind(id)
    .bind(format)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(lib, dir, stem, fmt)| {
        std::path::Path::new(&lib)
            .join(&dir)
            .join(format!("{stem}.{}", fmt.to_lowercase()))
    }))
}

/// Resolve the scan-root-relative directory of a book's file for the given
/// format (e.g. "EPUB"), as stored verbatim in `book_files.path` /
/// `books.path`. Used where a caller must not echo the server's absolute
/// filesystem layout back to the client (e.g. the OPF export response) but
/// still wants to show where the file lives *within* the library. Same
/// lowest-`ordinal` tie-break as [`book_file_path`]. `Ok(None)` when absent.
pub async fn book_file_relative_dir(
    pool: &SqlitePool,
    id: i64,
    format: &str,
) -> Result<Option<std::path::PathBuf>, super::BooksError> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT COALESCE(bf.path, b.path) \
         FROM books b \
         JOIN book_files bf ON bf.book_id = b.id \
         WHERE b.id = ? AND bf.format = ? COLLATE NOCASE \
         ORDER BY bf.ordinal LIMIT 1",
    )
    .bind(id)
    .bind(format)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(dir,)| std::path::PathBuf::from(dir)))
}

/// Resolve the on-disk path for a specific `book_files.id`, verifying
/// it belongs to the given book. When `format` is `Some`, also
/// validates the file's format matches (case-insensitive) — returns
/// `None` for mismatches so callers get a clean 404.
pub async fn book_file_path_by_id(
    pool: &SqlitePool,
    book_id: i64,
    book_file_id: i64,
    format: Option<&str>,
) -> Result<Option<std::path::PathBuf>, super::BooksError> {
    let sql = if format.is_some() {
        "SELECT COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON b.id = bf.book_id \
         JOIN scan_roots l ON l.id = b.library_id \
         WHERE bf.id = ?1 AND bf.book_id = ?2 AND bf.format = ?3 COLLATE NOCASE"
    } else {
        "SELECT COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON b.id = bf.book_id \
         JOIN scan_roots l ON l.id = b.library_id \
         WHERE bf.id = ?1 AND bf.book_id = ?2"
    };
    let mut q = sqlx::query_as::<_, (String, String, String, String)>(sql)
        .bind(book_file_id)
        .bind(book_id);
    if let Some(fmt) = format {
        q = q.bind(fmt);
    }
    let row = q.fetch_optional(pool).await?;
    Ok(row.map(|(lib, dir, stem, fmt)| {
        std::path::Path::new(&lib)
            .join(&dir)
            .join(format!("{stem}.{}", fmt.to_lowercase()))
    }))
}

/// Fetch all `book_files` rows for a book, ordered by format then ordinal.
pub async fn get_book_files(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Vec<omnibus_shared::BookFileInfo>, super::BooksError> {
    get_book_files_exec(pool, book_id).await
}

/// Executor-generic counterpart to [`get_book_files`], so a caller already
/// holding an open transaction (e.g. book deletion's count-inside-the-tx fix)
/// reads on the same connection its writes will run on. Pass `&pool` for a
/// standalone read or `&mut *tx` from within a transaction.
pub async fn get_book_files_exec<'e, E>(
    executor: E,
    book_id: i64,
) -> Result<Vec<omnibus_shared::BookFileInfo>, super::BooksError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            i64,
            Option<String>,
            i64,
            Option<String>,
            i64,
        ),
    >(
        "SELECT id, format, filename, ordinal, label, size_bytes, scan_key, mtime_epoch \
         FROM book_files WHERE book_id = ? ORDER BY format, ordinal",
    )
    .bind(book_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, format, filename, ordinal, label, size_bytes, path, mtime_epoch)| {
                omnibus_shared::BookFileInfo {
                    id,
                    format,
                    filename,
                    ordinal,
                    label,
                    size_bytes,
                    path,
                    // Same `(mtime_epoch, size_bytes)` pair the reindex diff
                    // keys on, so a file the indexer classified Changed is
                    // exactly a file whose validator moved.
                    etag: omnibus_shared::file_etag(size_bytes, mtime_epoch),
                }
            },
        )
        .collect())
}

/// Resolve a library-relative `scan_key` under the library rooted at
/// `library_path` to its book's durable `books.uuid`, or `None` if no book is
/// indexed at that path yet.
///
/// `scan_key` is the indexer's durable diff key (the library-relative path),
/// so the upload-commit handler can map a file it just filed back to the row a
/// reindex inserted without depending on the absolute path. A `None` result
/// means the triggered scan hasn't surfaced the file — the caller treats that
/// as a transient failure, not a 404.
pub async fn get_book_uuid_by_scan_key(
    pool: &SqlitePool,
    library_path: &str,
    scan_key: &str,
) -> Result<Option<String>, super::BooksError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT b.uuid FROM books b
         JOIN scan_roots sr ON sr.id = b.library_id
         WHERE sr.path = ?1 AND b.scan_key = ?2
         LIMIT 1",
    )
    .bind(library_path)
    .bind(scan_key)
    .fetch_optional(pool)
    .await?)
}

/// Answer a batch of "what is this file's validator now?" questions in two
/// queries, whatever the batch size.
///
/// The row picked per query mirrors what the download endpoints resolve: an
/// explicit `file_id` when given, else the lowest-ordinal row of a format
/// the download serves (`db::book_file_path`'s `ORDER BY bf.ordinal LIMIT
/// 1`, and `default_audio_file_id` on the client). Answering about a
/// different row than the one a download came from would report a book
/// stale that isn't.
///
/// `None` for an entry whose book, format, or chosen file is gone, or whose
/// row the scanner has not stat'd — every one of which a client reads as
/// "can't tell", never as "unchanged".
pub async fn download_validators(
    pool: &SqlitePool,
    queries: &[omnibus_shared::DownloadValidatorQuery],
) -> Result<Vec<omnibus_shared::DownloadValidator>, super::BooksError> {
    use std::collections::HashMap;

    // uuid → books.id, merged-uuid aware so an attached format still
    // resolves (migration 0016).
    let mut ids: HashMap<&str, i64> = HashMap::new();
    for query in queries {
        if ids.contains_key(query.book_uuid.as_str()) {
            continue;
        }
        if let Some(id) = crate::resolve_book_id_by_uuid(pool, &query.book_uuid).await? {
            ids.insert(query.book_uuid.as_str(), id);
        }
    }

    // One pass over every candidate row, chunked under SQLite's 999-param cap.
    let distinct: Vec<i64> = {
        let mut v: Vec<i64> = ids.values().copied().collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut rows_by_book: HashMap<i64, Vec<ValidatorRow>> = HashMap::new();
    for chunk in distinct.chunks(900) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT book_id, id, format, ordinal, size_bytes, mtime_epoch \
             FROM book_files WHERE book_id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, i64, String, i64, i64, i64)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for (book_id, id, format, ordinal, size_bytes, mtime_epoch) in q.fetch_all(pool).await? {
            rows_by_book.entry(book_id).or_default().push(ValidatorRow {
                id,
                format,
                ordinal,
                size_bytes,
                mtime_epoch,
            });
        }
    }

    Ok(queries
        .iter()
        .map(|query| {
            let etag = ids
                .get(query.book_uuid.as_str())
                .and_then(|book_id| rows_by_book.get(book_id))
                .and_then(|rows| pick_validator_row(rows, query))
                .and_then(|row| omnibus_shared::file_etag(row.size_bytes, row.mtime_epoch));
            omnibus_shared::DownloadValidator {
                book_uuid: query.book_uuid.clone(),
                format: query.format,
                file_id: query.file_id,
                etag,
            }
        })
        .collect())
}

/// The `book_files` columns a validator answer is built from.
struct ValidatorRow {
    id: i64,
    format: String,
    ordinal: i64,
    size_bytes: i64,
    mtime_epoch: i64,
}

/// The `book_files` row a query is about: the explicit `file_id` when given,
/// else the lowest-ordinal row of a format this download serves.
fn pick_validator_row<'a>(
    rows: &'a [ValidatorRow],
    query: &omnibus_shared::DownloadValidatorQuery,
) -> Option<&'a ValidatorRow> {
    if let Some(file_id) = query.file_id {
        return rows.iter().find(|row| row.id == file_id);
    }
    let formats = query.format.file_formats();
    rows.iter()
        .filter(|row| formats.iter().any(|f| f.eq_ignore_ascii_case(&row.format)))
        .min_by_key(|row| row.ordinal)
}
