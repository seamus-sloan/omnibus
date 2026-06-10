//! Single-book read paths: `get_book` (id), `get_book_by_uuid`, and the
//! `resolve_book_id_by_uuid` helper that the covers/thumbs/mobile routes
//! use to translate a stable uuid to the current id.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use crate::helpers::format_series_index;
use crate::metadata_overrides::{apply_overrides, get_metadata_overrides};

use super::projection::{
    backfill_creator_ids, parse_json_array, sanitize_description, CreatorRow, IdentifierRow,
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
    let row = sqlx::query(
        r#"
        SELECT
            b.id, b.uuid, b.title, b.description, b.series_index, b.has_cover,
            b.pubdate, b.last_modified, b.timestamp, b.accent_color,

            (SELECT bf.filename FROM book_files bf
              WHERE bf.book_id = b.id
              ORDER BY (bf.format != 'EPUB'), bf.format
              LIMIT 1)                                     AS primary_filename,

            (SELECT bf.format FROM book_files bf
              WHERE bf.book_id = b.id
              ORDER BY (bf.format != 'EPUB'), bf.format
              LIMIT 1)                                     AS primary_format,

            (SELECT pub.name FROM books_publishers_link bpl
              JOIN publishers pub ON pub.id = bpl.publisher
             WHERE bpl.book = b.id ORDER BY pub.name LIMIT 1)
                                                           AS publisher,

            (SELECT lang.code FROM books_languages_link bll
              JOIN languages lang ON lang.id = bll.language
             WHERE bll.book = b.id ORDER BY lang.code LIMIT 1)
                                                           AS language,

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
                      ORDER BY bal.position))              AS creators_json,

            (SELECT json_group_array(name)
               FROM (SELECT t.name AS name FROM books_tags_link btl
                       JOIN tags t ON t.id = btl.tag
                      WHERE btl.book = b.id
                      ORDER BY t.name))                    AS subjects_json,

            (SELECT json_group_array(json_object('scheme', scheme, 'value', value))
               FROM (SELECT scheme, value FROM book_identifiers
                      WHERE book_id = b.id
                      ORDER BY scheme, value))             AS identifiers_json,

            (SELECT json_group_array(format)
               FROM (SELECT format FROM book_files
                      WHERE book_id = b.id
                      ORDER BY format))                    AS formats_json

        FROM books b
        WHERE b.id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    let book_id: i64 = r.get("id");
    let has_cover: i64 = r.get("has_cover");
    let series_index: Option<f64> = r.get("series_index");
    let uuid: String = r.get("uuid");

    let filename = match (
        r.get::<Option<String>, _>("primary_filename"),
        r.get::<Option<String>, _>("primary_format"),
    ) {
        (Some(stem), Some(fmt)) => format!("{stem}.{}", fmt.to_ascii_lowercase()),
        _ => String::new(),
    };

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

    let formats: Vec<String> = parse_json_array(r.get("formats_json"))?;

    let mut book = EbookMetadata {
        id: book_id,
        filename,
        title: r.get("title"),
        description: sanitize_description(r.get("description")),
        publisher: r.get("publisher"),
        published: r.get("pubdate"),
        modified: r.get("last_modified"),
        language: r.get("language"),
        creators,
        subjects,
        identifiers,
        series: r.get("series_name"),
        series_index: series_index.map(format_series_index),
        series_id: r.get("series_link_id"),
        unique_identifier: Some(uuid.clone()),
        cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
        accent: r.get("accent_color"),
        formats,
        added_at: r.get("timestamp"),
        error: None,
        has_override: false,
    };

    // F5.1: merge user-supplied metadata overrides.
    if let Some((ov, has_cover_ov)) = get_metadata_overrides(pool, &uuid).await? {
        apply_overrides(&mut book, &uuid, &ov, has_cover_ov);
    }

    // `apply_overrides` rewrites `book.series` from the JSON blob but
    // can't touch the relational `books_series_link` row, so a book
    // whose series exists only as an override ends up with the series
    // *name* but no `series_id`. Backfill it by looking up the series
    // by name so the detail-page Link to /series/:id resolves.
    if book.series_id.is_none() {
        if let Some(name) = book.series.as_deref().filter(|s| !s.is_empty()) {
            book.series_id = sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
                .bind(name)
                .fetch_optional(pool)
                .await?;
        }
    }

    // Override Contributors are stored by name only — apply_overrides
    // therefore leaves `id` unset, which renders the breadcrumb /
    // "More by …" author link as an unclickable span even when an
    // `authors` row with that name exists. Backfill the id from the
    // authors table by name. Mirrors the series_id backfill above.
    backfill_creator_ids(pool, std::slice::from_mut(&mut book)).await?;

    Ok(Some(book))
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
/// The covers / thumbs / mobile-ebooks routes use this to keep their
/// URLs uuid-keyed externally while reusing the existing id-keyed
/// internal helpers (`get_cover`, the thumbnail pipeline) unchanged.
pub async fn resolve_book_id_by_uuid(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<i64>, super::BooksError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
            .bind(uuid)
            .fetch_optional(pool)
            .await?,
    )
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
    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT l.path, b.path, bf.filename FROM books b \
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
