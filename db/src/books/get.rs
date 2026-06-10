//! Single-book read paths: `get_book` (id), `get_book_by_uuid`, and the
//! `resolve_book_id_by_uuid` helper that the covers/thumbs/mobile routes
//! use to translate a stable uuid to the current id.

use sqlx::SqlitePool;

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

    let mut book = row_to_ebook(&row)?;
    let uuid = book.unique_identifier.clone().unwrap_or_default();

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
/// (e.g. "EPUB"). The indexer stores `books.path` **relative to its
/// `libraries.path` root** (mirroring the scanner's `root.join(filename)`),
/// and `book_files.filename` as the stem, so the path is
/// `<libraries.path>/<books.path>/<filename>.<format-lowercased>`. When the
/// library root is itself relative the result resolves against the server's
/// working directory, exactly as the scanner read it. Ok(None) when the book
/// or a file row for that format is absent.
pub async fn book_file_path(
    pool: &SqlitePool,
    id: i64,
    format: &str,
) -> Result<Option<std::path::PathBuf>, super::BooksError> {
    // COALESCE: an attached / merged file row carries its own
    // `(library_path, path)` override because its on-disk home is not
    // the book's library (see migration 0016).
    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), bf.filename \
         FROM books b \
         JOIN libraries l ON l.id = b.library_id \
         JOIN book_files bf ON bf.book_id = b.id \
         WHERE b.id = ? AND bf.format = ? COLLATE NOCASE LIMIT 1",
    )
    .bind(id)
    .bind(format)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(lib, dir, stem)| {
        std::path::Path::new(&lib)
            .join(&dir)
            .join(format!("{stem}.{}", format.to_lowercase()))
    }))
}
