//! Book read path. Hydrates the normalized schema into the wire
//! `EbookMetadata` shape: scalar columns from `books`, single-valued joins
//! via scalar subqueries, multi-valued joins via `json_group_array`. Merges
//! `metadata_overrides` and backfills creator ids before returning.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata, Identifier};

use crate::helpers::{build_fts_match, format_series_index};
use crate::metadata_overrides::{apply_overrides, get_metadata_overrides, load_overrides_bulk};

/// Hard server-side cap on the number of books any single list/search
/// response returns. The F1.3 spec ([docs/roadmap/1-3-library-views.md])
/// allows client-side sort/filter "for libraries up to ~10k books", but
/// nothing previously enforced that — `list_books` / `search_books`
/// streamed the entire library on every request, so a multi-thousand-book
/// install paid the serialization cost on every poll. Issue #81.
///
/// 50k is well above the spec's client-side ceiling (anything beyond
/// needs server-side pagination anyway) and small enough that JSON-
/// encoding the response stays in a sensible memory envelope.
///
/// Callers that need the *full* count (for "X books truncated" UI or the
/// `X-Total-Count` header) should reach for `library_from_db_with_total`
/// / `count_search_books`, which return the underlying count alongside
/// the capped vec. Cursor-based pagination is intentionally deferred.
pub const MAX_BOOKS_RETURNED: i64 = 50_000;

/// The shared book-column SELECT for read-path queries. Same scalar-subquery
/// pattern as `list_books` / `search_books` — one row per `books.id` with
/// all m2m relations inlined as JSON aggregates. Re-used by the discovery
/// read paths (`get_author`, `get_series`) via `pub(crate)`.
pub(crate) const BOOK_COLUMNS: &str = r#"
    b.id, b.uuid,
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
              ORDER BY bal.position))             AS creators_json,

    (SELECT json_group_array(name)
       FROM (SELECT t.name AS name FROM books_tags_link btl
               JOIN tags t ON t.id = btl.tag
              WHERE btl.book = b.id
              ORDER BY t.name))                   AS subjects_json,

    (SELECT json_group_array(json_object('scheme', scheme, 'value', value))
       FROM (SELECT scheme, value FROM book_identifiers
              WHERE book_id = b.id
              ORDER BY scheme, value))            AS identifiers_json,

    (SELECT json_group_array(format)
       FROM (SELECT format FROM book_files
              WHERE book_id = b.id
              ORDER BY format))                   AS formats_json
"#;

#[derive(serde::Deserialize)]
pub(crate) struct CreatorRow {
    #[serde(default)]
    pub(crate) id: Option<i64>,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) sort: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct IdentifierRow {
    pub(crate) scheme: String,
    pub(crate) value: String,
}

/// Decode a `json_group_array` blob produced by SQLite. Returns `"[]"` for an
/// aggregate over zero rows, so a `None` here only means the column itself was
/// NULL (which the subqueries never produce, but we tolerate it defensively).
pub(crate) fn parse_json_array<T: serde::de::DeserializeOwned>(
    blob: Option<String>,
) -> Result<Vec<T>, sqlx::Error> {
    match blob {
        Some(s) => serde_json::from_str(&s).map_err(|e| sqlx::Error::Decode(Box::new(e))),
        None => Ok(Vec::new()),
    }
}

/// Sanitize an EPUB `<dc:description>` payload for safe rendering via
/// `dangerous_inner_html`. OPF descriptions are commonly HTML fragments
/// (`<p>`, `<b>`, `<em>`, lists, links). We rely on ammonia's default
/// allowlist: it strips `<script>`, event handlers, `javascript:` URLs,
/// `<style>`, `<iframe>`, etc., while preserving inline formatting.
///
/// Applied at read time in `get_book` so existing DB rows benefit without a
/// reindex. Empty/whitespace-only output collapses to `None` so the book
/// detail page hides the description block entirely.
pub(crate) fn sanitize_description(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let cleaned = ammonia::clean(&raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Convert a SQLite row (selected with [`BOOK_COLUMNS`]) into an
/// [`EbookMetadata`]. Shared across the discovery query functions.
pub(crate) fn row_to_ebook(r: &sqlx::sqlite::SqliteRow) -> Result<EbookMetadata, sqlx::Error> {
    let id: i64 = r.get("id");
    let uuid: String = r.get("uuid");
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

    Ok(EbookMetadata {
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
    })
}

/// Return every book indexed under `library_path`. One round-trip to SQLite:
/// every multi-valued relation is pulled in a single statement using scalar
/// subqueries (for single-valued joins) and `json_group_array` over ordered
/// inner selects (for multi-valued lists), matching the pattern in `get_book`.
pub async fn list_books(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
    let rows = sqlx::query(
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
        WHERE l.path = ?
        ORDER BY b.sort, b.id
        LIMIT ?
        "#,
    )
    .bind(library_path)
    .bind(MAX_BOOKS_RETURNED)
    .fetch_all(pool)
    .await?;

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
            description: r.get("description"),
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
        if let Some(uuid) = book.unique_identifier.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, ov, *has_cover_ov);
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

/// Total number of books currently indexed under `library_path`.
///
/// Companion to `list_books`: `list_books` caps the returned vec at
/// `MAX_BOOKS_RETURNED`, so callers that need to surface a truncation
/// hint (UI banner, `X-Total-Count` header) ask the count separately.
/// Single scalar query — cheaper than re-running the full SELECT just
/// to count rows. Issue #81.
pub async fn count_books(pool: &SqlitePool, library_path: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
          FROM books b
          JOIN libraries l ON l.id = b.library_id
         WHERE l.path = ?
        "#,
    )
    .bind(library_path)
    .fetch_one(pool)
    .await
}

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
pub async fn get_book(pool: &SqlitePool, id: i64) -> Result<Option<EbookMetadata>, sqlx::Error> {
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
        formats,
        added_at: r.get("timestamp"),
        error: None,
        has_override: false,
    };

    // F5.1: merge user-supplied metadata overrides.
    if let Some((ov, has_cover_ov)) = get_metadata_overrides(pool, &uuid).await? {
        apply_overrides(&mut book, &ov, has_cover_ov);
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
) -> Result<Option<EbookMetadata>, sqlx::Error> {
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
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_optional(pool)
        .await
}

/// Fill in `Contributor::id` for every creator whose `id` is `None` by
/// looking up the `authors` table by exact name. Used after the override
/// merge in `get_book`, `list_books`, `get_author`, and `get_series` so
/// the UI's `Route::AuthorDetail { id }` link resolves for books whose
/// authors were renamed (or replaced) through `metadata_overrides`.
pub(crate) async fn backfill_creator_ids(
    pool: &SqlitePool,
    books: &mut [EbookMetadata],
) -> Result<(), sqlx::Error> {
    use std::collections::HashMap;

    // Collect every distinct name that still needs an id.
    let names: Vec<String> = {
        let mut set = std::collections::HashSet::new();
        for b in books.iter() {
            for c in &b.creators {
                if c.id.is_none() && !c.name.is_empty() {
                    set.insert(c.name.clone());
                }
            }
        }
        set.into_iter().collect()
    };
    if names.is_empty() {
        return Ok(());
    }

    // Bulk lookup in chunks to stay under SQLite's bound-parameter limit.
    //
    // `authors.name` is `UNIQUE COLLATE NOCASE`, so the SQL `WHERE name IN (...)`
    // matches case-insensitively — but the returned row carries the DB casing,
    // and an override's `Contributor::name` carries the user-supplied casing.
    // Key the map by `to_lowercase()` on both sides so an override like
    // "ada lovelace" still resolves to the canonical "Ada Lovelace" row's id.
    let mut name_to_id: HashMap<String, i64> = HashMap::with_capacity(names.len());
    for chunk in names.chunks(500) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, name FROM authors WHERE name IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for n in chunk {
            q = q.bind(n);
        }
        let rows = q.fetch_all(pool).await?;
        for r in rows {
            let db_name: String = r.get("name");
            name_to_id.insert(db_name.to_lowercase(), r.get("id"));
        }
    }

    for b in books.iter_mut() {
        for c in &mut b.creators {
            if c.id.is_none() {
                c.id = name_to_id.get(&c.name.to_lowercase()).copied();
            }
        }
    }
    Ok(())
}

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
    let Some(match_expr) = build_fts_match(q) else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(
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
        FROM books_fts
        JOIN books b ON b.id = books_fts.rowid
        JOIN libraries l ON l.id = b.library_id
        WHERE books_fts MATCH ? AND l.path = ?
        ORDER BY bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0), b.sort, b.id
        LIMIT ?
        "#,
    )
    .bind(&match_expr)
    .bind(library_path)
    .bind(MAX_BOOKS_RETURNED)
    .fetch_all(pool)
    .await?;

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
            description: r.get("description"),
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
        if let Some(uuid) = book.unique_identifier.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, ov, *has_cover_ov);
            }
        }
    }
    backfill_creator_ids(pool, &mut out).await?;

    Ok(out)
}

/// Total number of FTS5 hits for `q` under `library_path` (before the
/// `MAX_BOOKS_RETURNED` cap is applied). Empty/whitespace `q` returns 0
/// to mirror `search_books`. Issue #81.
pub async fn count_search_books(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<i64, sqlx::Error> {
    let Some(match_expr) = build_fts_match(q) else {
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

/// Build an `EbookLibrary` from whatever is currently in the DB for
/// `library_path`. Returns an empty library (no error, no books) if the path
/// is `None`.
///
/// The returned `books` vec is capped at `MAX_BOOKS_RETURNED` (issue #81);
/// callers that need to surface a truncation hint should use
/// [`library_from_db_with_total`] instead. This entrypoint deliberately
/// avoids the extra `count_books` round-trip — non-REST callers (the RPC
/// path, internal lookups) don't need the total and shouldn't pay for it.
pub async fn library_from_db(
    pool: &SqlitePool,
    library_path: Option<&str>,
) -> Result<EbookLibrary, sqlx::Error> {
    let Some(path) = library_path else {
        return Ok(EbookLibrary::default());
    };
    let books = list_books(pool, path).await?;
    Ok(EbookLibrary {
        path: Some(path.to_string()),
        books,
        error: None,
    })
}

/// Same as `library_from_db` but also returns the *true* book count under
/// `library_path` (before the `MAX_BOOKS_RETURNED` cap). Used by the REST
/// handler to set `X-Total-Count` and `X-Total-Cap` response headers so
/// the client can detect a truncated response. Issue #81.
pub async fn library_from_db_with_total(
    pool: &SqlitePool,
    library_path: Option<&str>,
) -> Result<(EbookLibrary, i64), sqlx::Error> {
    let Some(path) = library_path else {
        return Ok((EbookLibrary::default(), 0));
    };
    let books = list_books(pool, path).await?;
    let total = count_books(pool, path).await?;
    Ok((
        EbookLibrary {
            path: Some(path.to_string()),
            books,
            error: None,
        },
        total,
    ))
}
