//! Single-book read paths: `get_book` (id), `get_book_by_uuid`, and the
//! `resolve_book_id_by_uuid` helper that the covers/thumbs/mobile routes
//! use to translate a stable uuid to the current id.

use sqlx::{Row, SqlitePool};

use omnibus_shared::EbookMetadata;

use crate::metadata_overrides::{apply_overrides, get_metadata_overrides};

use super::projection::{backfill_creator_ids, row_to_ebook, BOOK_COLUMNS};

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

    // F5.1: merge user-supplied metadata overrides.
    if let Some((ov, has_cover_ov)) = get_metadata_overrides(pool, &uuid).await? {
        apply_overrides(&mut book, &uuid, &ov, has_cover_ov);
    }

    backfill_series_id_by_name(pool, &mut book).await?;
    // Override Contributors are stored by name only — apply_overrides
    // therefore leaves `id` unset, which renders the breadcrumb /
    // "More by …" author link as an unclickable span even when an
    // `authors` row with that name exists. Backfill the id from the
    // authors table by name. Mirrors the series_id backfill above.
    backfill_creator_ids(pool, std::slice::from_mut(&mut book)).await?;

    let files = get_book_files(pool, id).await?;
    let has_multipart = files.iter().any(|f| {
        files
            .iter()
            .filter(|o| o.format.eq_ignore_ascii_case(&f.format))
            .count()
            > 1
    });
    if has_multipart {
        book.book_files = files;
    }

    Ok(Some(book))
}

/// Run the single-row `SELECT … WHERE b.id = ?` against `books` with the
/// shared `BOOK_COLUMNS` projection. Returns `None` if the id is unknown.
async fn fetch_book_row(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> {
    let sql = format!(
        r#"
        SELECT {BOOK_COLUMNS}
        FROM books b
        WHERE b.id = ?
        "#
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
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM books WHERE uuid = ?1
         UNION ALL
         SELECT book_id FROM merged_uuids WHERE uuid = ?1
         LIMIT 1",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await?)
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
         JOIN libraries l ON l.id = b.library_id \
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
         JOIN libraries l ON l.id = b.library_id \
         WHERE bf.id = ?1 AND bf.book_id = ?2 AND bf.format = ?3 COLLATE NOCASE"
    } else {
        "SELECT COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON b.id = bf.book_id \
         JOIN libraries l ON l.id = b.library_id \
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
    let rows = sqlx::query_as::<_, (i64, String, String, i64, Option<String>)>(
        "SELECT id, format, filename, ordinal, label FROM book_files \
         WHERE book_id = ? ORDER BY format, ordinal",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, format, filename, ordinal, label)| omnibus_shared::BookFileInfo {
                id,
                format,
                filename,
                ordinal,
                label,
            },
        )
        .collect())
}
