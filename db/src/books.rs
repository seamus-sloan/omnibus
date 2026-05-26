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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::covers::test_helpers::CoversTempDir;
    use crate::discovery::test_helpers::{
        author_id_by_name, seed_discovery_fixture, series_id_by_name,
    };
    use crate::ebook::IndexedBook;
    use crate::metadata_overrides::upsert_metadata_overrides;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::sync::test_helpers::indexed;
    use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};
    use sqlx::SqlitePool;

    // ---------- Server-side cap (issue #81) ----------
    //
    // `list_books` / `search_books` previously had no `LIMIT`, so a single
    // `/api/ebooks` poll on a multi-thousand-book library serialized the
    // whole table. The fix is a hard `LIMIT MAX_BOOKS_RETURNED`, plus a
    // companion count helper so callers can detect truncation.

    /// Seed `count` minimal `books` rows under `/lib` using a recursive CTE.
    /// Bypasses `replace_books` / the indexer entirely — the cap behavior
    /// only depends on rows existing, not on full m2m relations being set
    /// up. Keeps the test runtime down to milliseconds even for 50k+ rows.
    async fn seed_minimal_books(pool: &SqlitePool, count: i64) {
        sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
            .execute(pool)
            .await
            .unwrap();
        let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
            .fetch_one(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1
                UNION ALL
                SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i)
              FROM n
            "#,
        )
        .bind(count)
        .bind(lib_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn library_from_db_returns_empty_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib = library_from_db(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }
    #[tokio::test]
    async fn list_books_caps_response_at_max_books_returned() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_BOOKS_RETURNED + 25;
        seed_minimal_books(&pool, total).await;

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(
            books.len() as i64,
            MAX_BOOKS_RETURNED,
            "list_books must cap the returned vec at MAX_BOOKS_RETURNED"
        );

        let counted = count_books(&pool, "/lib").await.unwrap();
        assert_eq!(
            counted, total,
            "count_books must report the true row count (uncapped)"
        );
    }
    #[tokio::test]
    async fn count_books_returns_zero_for_unknown_library() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        assert_eq!(count_books(&pool, "/nope").await.unwrap(), 0);
    }
    #[tokio::test]
    async fn library_from_db_with_total_reports_truncation() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_BOOKS_RETURNED + 7;
        seed_minimal_books(&pool, total).await;

        let (lib, returned_total) = library_from_db_with_total(&pool, Some("/lib"))
            .await
            .unwrap();
        assert_eq!(lib.path.as_deref(), Some("/lib"));
        assert_eq!(lib.books.len() as i64, MAX_BOOKS_RETURNED);
        assert_eq!(returned_total, total);
        assert!(
            returned_total > MAX_BOOKS_RETURNED,
            "test must seed strictly more rows than the cap"
        );
    }
    #[tokio::test]
    async fn library_from_db_with_total_reports_zero_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let (lib, total) = library_from_db_with_total(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert_eq!(total, 0);
    }
    #[tokio::test]
    async fn search_books_finds_by_title_and_ranks_by_bm25() {
        let _covers = CoversTempDir::new("fts_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Harry Potter"),
                    &["J.K. Rowling"],
                    &[],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Something Else"),
                    &["Author B"],
                    &["harry"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "harry").await.unwrap();
        // Column filter scopes MATCH to title/authors/series — the tag-only
        // hit on "Something Else" is intentionally excluded.
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Harry Potter"));
    }
    #[tokio::test]
    async fn search_books_finds_by_author_and_scopes_to_library() {
        let _covers = CoversTempDir::new("fts_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-a", "tolkien").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }
    #[tokio::test]
    async fn search_books_empty_query_returns_empty_vec() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = search_books(&pool, "/lib", "   ").await.unwrap();
        assert!(hits.is_empty());
    }
    #[tokio::test]
    async fn search_books_handles_unbalanced_quote_without_error() {
        let _covers = CoversTempDir::new("fts_unbalanced");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Quoted"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Unbalanced `"` in raw input must not surface as a MATCH parse error.
        let hits = search_books(&pool, "/lib", "say \"hi")
            .await
            .expect("sanitizer guards against MATCH parse errors");
        assert!(hits.is_empty());
    }
    #[tokio::test]
    async fn search_books_excludes_isbn_column_from_match() {
        // ISBN is indexed in books_fts but the search column filter scopes
        // MATCH to title/authors/series, so ISBN lookups are intentionally
        // not surfaced. When/if we re-enable ISBN search, flip this to
        // assert a hit — no migration required.
        let _covers = CoversTempDir::new("fts_isbn");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let mut meta = indexed("a.epub", Some("ISBN Book"), &["A"], &[], None, None).metadata;
        meta.identifiers.push(Identifier {
            value: "978-0-123456-78-9".into(),
            scheme: Some("isbn".into()),
        });
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: meta,
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "978-0-123456-78-9")
            .await
            .unwrap();
        assert!(hits.is_empty());
    }
    #[tokio::test]
    async fn search_books_author_facet_filters_to_matching_author() {
        let _covers = CoversTempDir::new("fts_facet_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Pride and Prejudice"),
                    &["Austen"],
                    &[],
                    None,
                    None,
                ),
                indexed("b.epub", Some("Hamlet"), &["Shakespeare"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "author:austen").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
    }
    #[tokio::test]
    async fn search_books_series_facet_filters_to_matching_series() {
        let _covers = CoversTempDir::new("fts_facet_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dune"),
                    &["Herbert"],
                    &[],
                    Some(("Dune Saga", "1")),
                    None,
                ),
                indexed("b.epub", Some("Standalone"), &["Author"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "series:dune").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Dune"));
    }
    #[tokio::test]
    async fn search_books_tag_facet_filters_to_matching_tag() {
        let _covers = CoversTempDir::new("fts_facet_tag");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
                indexed("b.epub", Some("B"), &["Y"], &["history"], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "tag:fiction").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }
    #[tokio::test]
    async fn search_books_facet_combines_with_free_text_via_explicit_and() {
        let _covers = CoversTempDir::new("fts_facet_combined");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Pride and Prejudice"),
                    &["Austen"],
                    &[],
                    None,
                    None,
                ),
                indexed("b.epub", Some("Emma"), &["Austen"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // Both clauses must match — only Pride and Prejudice carries the
        // "pride" token in title/authors/series.
        let hits = search_books(&pool, "/lib", "author:austen pride")
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
    }
    #[tokio::test]
    async fn list_books_populates_formats_from_book_files() {
        // Regression: F1.7 power-user table & inline format chips read
        // `EbookMetadata.formats` off the list endpoint; if list_books
        // returned `vec![]` the chip row would hide itself entirely.
        let _covers = CoversTempDir::new("list_books_formats");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].formats, vec!["EPUB".to_string()]);
    }
    #[tokio::test]
    async fn list_books_returns_one_row_per_book_with_multi_format() {
        // Regression for PR #74 review: adding a second physical file
        // (EPUB + M4B) used to duplicate the parent row because the outer
        // query LEFT-JOINed `book_files`. The chip facets / table would
        // then over-count. Both queries now use scalar subqueries so the
        // result is one row per `books.id` and `.formats` carries both.
        let _covers = CoversTempDir::new("list_books_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1, "multi-format must not duplicate rows");
        assert_eq!(
            books[0].formats,
            vec!["EPUB".to_string(), "M4B".to_string()]
        );
        // EPUB wins the primary-filename tiebreak, matching get_book.
        assert_eq!(books[0].filename, "alpha.epub");
    }
    #[tokio::test]
    async fn search_books_returns_one_row_per_book_with_multi_format() {
        // Same regression in the FTS path.
        let _covers = CoversTempDir::new("search_books_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let results = search_books(&pool, "/lib", "Alpha").await.unwrap();
        assert_eq!(results.len(), 1, "FTS results must not duplicate rows");
        assert_eq!(
            results[0].formats,
            vec!["EPUB".to_string(), "M4B".to_string()]
        );
    }
    #[tokio::test]
    async fn list_books_filters_by_author_join() {
        let _covers = CoversTempDir::new("filter_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["Pratchett"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let titles: Vec<String> = sqlx::query_scalar(
            "SELECT b.title FROM books b
             JOIN books_authors_link bal ON bal.book = b.id
             JOIN authors a ON a.id = bal.author
             WHERE a.name = ?",
        )
        .bind("Tolkien")
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(titles, vec!["A".to_string()]);
    }
    /// Regression guard for the N+1 fix in list_books / search_books.
    /// Both functions must return all creators, subjects, and identifiers
    /// via the inline json_group_array subqueries rather than per-book
    /// round-trip calls.
    #[tokio::test]
    async fn list_and_search_books_return_multi_valued_fields() {
        let _covers = CoversTempDir::new("multi_valued");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "multi.epub".into(),
                    title: Some("Multi".into()),
                    creators: vec![
                        Contributor {
                            name: "Alice".into(),
                            ..Default::default()
                        },
                        Contributor {
                            name: "Bob".into(),
                            ..Default::default()
                        },
                    ],
                    subjects: vec!["Fiction".into(), "Sci-Fi".into()],
                    identifiers: vec![
                        Identifier {
                            value: "978-0-000000-00-0".into(),
                            scheme: Some("isbn".into()),
                        },
                        Identifier {
                            value: "https://example.com/book/1".into(),
                            scheme: Some("uri".into()),
                        },
                    ],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let book = &books[0];
        assert_eq!(
            book.creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert_eq!(
            book.subjects,
            vec!["Fiction".to_string(), "Sci-Fi".to_string()]
        );
        assert_eq!(book.identifiers.len(), 2);
        assert_eq!(book.identifiers[0].scheme.as_deref(), Some("isbn"));
        assert_eq!(book.identifiers[1].scheme.as_deref(), Some("uri"));

        let hits = search_books(&pool, "/lib", "Multi").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0]
                .creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert_eq!(
            hits[0].subjects,
            vec!["Fiction".to_string(), "Sci-Fi".to_string()]
        );
        assert_eq!(hits[0].identifiers.len(), 2);
    }
    #[tokio::test]
    async fn get_book_returns_none_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let result = get_book(&pool, 9999).await.unwrap();
        assert!(result.is_none());
    }
    #[tokio::test]
    async fn get_book_is_deterministic_with_multiple_files_and_links() {
        // Regression for PR #55 review: when a book has multiple `book_files`
        // rows (and incidental duplicate publisher/language/series links),
        // get_book() must return the EPUB-preferred filename and stable
        // publisher/language/series values rather than whichever joined row
        // SQLite happens to return first.
        let _covers = CoversTempDir::new("get_book_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha Book"),
                &["Author A"],
                &["Fiction"],
                Some(("Saga", "1")),
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        // Add a second physical file in another format.
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Add a second publisher and a second language to exercise the
        // multi-row JOIN path on those link tables. Series already has one
        // row from `replace_books`.
        sqlx::query("INSERT INTO publishers (name) VALUES ('Acme'), ('Zenith')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books_publishers_link (book, publisher)
             SELECT ?, id FROM publishers WHERE name IN ('Acme', 'Zenith')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO languages (code) VALUES ('eng'), ('fra')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books_languages_link (book, language)
             SELECT ?, id FROM languages WHERE code IN ('eng', 'fra')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Run get_book repeatedly — every call must return identical values.
        let first = get_book(&pool, id).await.unwrap().expect("book");
        for _ in 0..3 {
            let again = get_book(&pool, id).await.unwrap().expect("book");
            assert_eq!(again.filename, first.filename);
            assert_eq!(again.publisher, first.publisher);
            assert_eq!(again.language, first.language);
            assert_eq!(again.series, first.series);
            assert_eq!(again.formats, first.formats);
        }

        // EPUB should win the tiebreak for filename.
        assert_eq!(first.filename, "alpha.epub");
        // Both formats surface in the formats list, sorted by format code.
        assert_eq!(first.formats, vec!["EPUB".to_string(), "M4B".to_string()]);
        // Publisher/language pick alphabetical winners deterministically.
        assert_eq!(first.publisher.as_deref(), Some("Acme"));
        assert_eq!(first.language.as_deref(), Some("eng"));
        assert_eq!(first.series.as_deref(), Some("Saga"));
        assert_eq!(first.series_index.as_deref(), Some("1"));
    }
    #[tokio::test]
    async fn get_book_returns_metadata_for_indexed_book() {
        let _covers = CoversTempDir::new("get_book");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha Book"),
                &["Author A"],
                &["Fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        let book = get_book(&pool, id)
            .await
            .unwrap()
            .expect("book should exist");
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Alpha Book"));
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, "Author A");
        assert_eq!(book.subjects, vec!["Fiction"]);
        assert!(!book.formats.is_empty(), "formats should be populated");
        assert!(
            book.formats.iter().any(|f| f.eq_ignore_ascii_case("epub")),
            "EPUB format should be present"
        );
    }
    #[tokio::test]
    async fn get_book_handles_book_with_no_relations() {
        // A `books` row that has zero m2m link rows and zero files: every
        // `json_group_array` subquery returns "[]" (over zero inner rows) and
        // every scalar subquery returns NULL. The function must still return
        // a populated `EbookMetadata` with empty vecs and an empty filename
        // rather than erroring out on the missing data.
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib_res =
            sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
                .execute(&pool)
                .await
                .unwrap();
        let lib_id = lib_res.last_insert_rowid();
        let res = sqlx::query(
            "INSERT INTO books (uuid, library_id, path, title) \
             VALUES ('lonely-uuid', ?, '/lib/lonely', 'Lonely')",
        )
        .bind(lib_id)
        .execute(&pool)
        .await
        .unwrap();
        let id = res.last_insert_rowid();

        let book = get_book(&pool, id)
            .await
            .unwrap()
            .expect("book should exist");
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Lonely"));
        assert_eq!(book.filename, "");
        assert!(book.creators.is_empty());
        assert!(book.subjects.is_empty());
        assert!(book.identifiers.is_empty());
        assert!(book.formats.is_empty());
        assert!(book.publisher.is_none());
        assert!(book.language.is_none());
        assert!(book.series.is_none());
        assert!(book.cover_url.is_none());
    }
    #[tokio::test]
    async fn get_book_round_trips_values_containing_control_chars_and_quotes() {
        // Regression for PR #65 review: prior delimiter-based encoding
        // (`GROUP_CONCAT` with 0x1F/0x1E separators) would have silently
        // corrupted any value containing those control chars. The JSON
        // encoding must survive arbitrary UTF-8 — control chars, quotes,
        // backslashes, commas — without altering the round-tripped value.
        let _covers = CoversTempDir::new("get_book_collide");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let nasty_name = "Smith\u{001F}, John \"O'Reilly\" \\back\u{001E}/";
        let nasty_tag = "Sci-Fi\u{001F}Drama";
        let nasty_value = "9780\u{001E}123\"456\\";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &[nasty_name],
                &[nasty_tag],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query("INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?, 'ISBN', ?)")
            .bind(id)
            .bind(nasty_value)
            .execute(&pool)
            .await
            .unwrap();

        let book = get_book(&pool, id).await.unwrap().expect("book");
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, nasty_name);
        assert_eq!(book.subjects, vec![nasty_tag.to_string()]);
        assert_eq!(book.identifiers.len(), 1);
        assert_eq!(book.identifiers[0].value, nasty_value);
        assert_eq!(book.identifiers[0].scheme.as_deref(), Some("ISBN"));
    }
    #[test]
    fn sanitize_description_preserves_safe_html() {
        let cleaned = sanitize_description(Some(
            "<p>Hello <strong>world</strong>!</p><p>Second <em>line</em>.</p>".into(),
        ))
        .unwrap();
        assert!(cleaned.contains("<p>"));
        assert!(cleaned.contains("<strong>world</strong>"));
        assert!(cleaned.contains("<em>line</em>"));
    }
    #[test]
    fn sanitize_description_strips_scripts_and_event_handlers() {
        // ammonia's defaults must drop <script>, inline `onerror`, and
        // `javascript:` URLs. Anything that could execute on the detail page
        // when rendered via dangerous_inner_html is the threat model here.
        let cleaned = sanitize_description(Some(
            "<p>Safe</p><script>alert('xss')</script>\
             <img src=x onerror=\"alert(1)\"/>\
             <a href=\"javascript:alert(1)\">click</a>"
                .into(),
        ))
        .unwrap();
        assert!(!cleaned.contains("<script"));
        assert!(!cleaned.to_ascii_lowercase().contains("onerror"));
        assert!(!cleaned.to_ascii_lowercase().contains("javascript:"));
        assert!(cleaned.contains("<p>Safe</p>"));
    }
    #[test]
    fn sanitize_description_collapses_empty_input_to_none() {
        assert_eq!(sanitize_description(None), None);
        assert_eq!(sanitize_description(Some(String::new())), None);
        assert_eq!(sanitize_description(Some("   \n\t".into())), None);
        // A bare <script> with no other content sanitizes to "" and must
        // collapse to None so the UI hides the description block entirely.
        assert_eq!(
            sanitize_description(Some("<script>alert(1)</script>".into())),
            None
        );
    }
    #[tokio::test]
    async fn get_book_returns_sanitized_html_description() {
        let _covers = CoversTempDir::new("sanitize_desc");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let raw =
            "<p>Brief.</p><script>alert('xss')</script><p>More <b>detail</b>.</p>".to_string();
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("Alpha".into()),
                    description: Some(raw),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;

        let desc = get_book(&pool, id).await.unwrap().unwrap().description;
        let desc = desc.expect("description should be present");
        assert!(desc.contains("<p>Brief.</p>"));
        assert!(desc.contains("<b>detail</b>"));
        assert!(!desc.contains("<script"));
    }
    #[tokio::test]
    async fn get_book_merges_scalar_overrides() {
        let _covers = CoversTempDir::new("merge_scalar");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "merge.epub",
                Some("Original Title"),
                &["Author A"],
                &["fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let book = &books[0];
        let uuid = book.unique_identifier.clone().unwrap();
        let id = book.id;

        // Save overrides.
        let ov = MetadataOverrides {
            title: Some("Edited Title".into()),
            publisher: Some("New Publisher".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // get_book should return merged values.
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.title.as_deref(), Some("Edited Title"));
        assert_eq!(merged.publisher.as_deref(), Some("New Publisher"));
        assert!(merged.has_override);
        // Non-overridden fields unchanged.
        assert_eq!(merged.creators[0].name, "Author A");
    }
    #[tokio::test]
    async fn get_book_merges_creators_replaces_entirely() {
        let _covers = CoversTempDir::new("merge_creators");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "creators.epub",
                Some("Book"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        let ov = MetadataOverrides {
            creators: Some(vec![
                Contributor {
                    name: "Author B".into(),
                    ..Default::default()
                },
                Contributor {
                    name: "Author C".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 2);
        assert_eq!(merged.creators[0].name, "Author B");
        assert_eq!(merged.creators[1].name, "Author C");
    }
    #[tokio::test]
    async fn get_book_merges_subjects_replaces_entirely() {
        let _covers = CoversTempDir::new("merge_subjects");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "subjects.epub",
                Some("Book"),
                &["Author"],
                &["fiction", "classic"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        let ov = MetadataOverrides {
            subjects: Some(vec!["sci-fi".into(), "adventure".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.subjects, vec!["sci-fi", "adventure"]);
    }
    #[tokio::test]
    async fn get_book_backfills_creator_ids_after_override_replaces_authors() {
        // Override Contributors carry only a name, so a book whose author
        // list was edited through the metadata form would otherwise come
        // back with `creators[*].id == None`, rendering the breadcrumb's
        // author link as an unclickable span even when the `authors` row
        // exists. Verify get_book backfills the id by name. Mirrors the
        // user's report against book 268 (multi-author book where the
        // user removed all but one canonical author).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // saga1.epub canonically has ["Ada Lovelace", "Grace Hopper"];
        // simulate the user dropping the second author through the edit
        // form. apply_overrides replaces creators wholesale, so the
        // override Contributor has id = None.
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Ada Lovelace".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "Ada Lovelace");
        assert_eq!(
            merged.creators[0].id,
            Some(ada_id),
            "creator id must be backfilled so the breadcrumb renders as a Link",
        );
    }
    #[tokio::test]
    async fn get_book_backfills_creator_ids_case_insensitively() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`, so a SQL `IN (...)`
        // lookup matches case-insensitively — but the returned row carries
        // the DB casing while the override carries the user-supplied
        // casing. The HashMap must normalise both sides to lowercase so
        // an override like "ada lovelace" still resolves to the canonical
        // "Ada Lovelace" id.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ADA LOVELACE".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "ADA LOVELACE");
        assert_eq!(
            merged.creators[0].id,
            Some(ada_id),
            "case-mismatched override should still resolve to the canonical author id",
        );
    }
    #[tokio::test]
    async fn get_book_leaves_creator_id_none_when_override_author_unknown() {
        // If the override sets an author name that doesn't exist in the
        // `authors` table, backfill must leave the id None — same shape
        // as get_book_leaves_series_id_none_when_override_series_unknown.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Nobody Indexed".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "Nobody Indexed");
        assert_eq!(merged.creators[0].id, None);
    }
    #[tokio::test]
    async fn get_book_backfills_series_id_from_override_when_series_exists() {
        // A book whose series was set via overrides (not at scan time)
        // historically came back with series_id == None even though the
        // series row existed in the relational table. The detail page's
        // "Series" rail then fell back to plain text instead of a Link
        // to /series/:id. Verify the read path now backfills the id.
        let _covers = CoversTempDir::new("override_series_link");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Seed: one book belongs to "Saga" natively (so the series row exists),
        // one standalone book that we'll later override into the same series.
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "saga1.epub",
                    Some("Saga: Book One"),
                    &["Author X"],
                    &[],
                    Some(("Saga", "1")),
                    None,
                ),
                indexed("loner.epub", Some("Loner"), &["Author Y"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let saga_id = series_id_by_name(&pool, "Saga").await;
        let books = list_books(&pool, "/lib").await.unwrap();
        let loner = books.iter().find(|b| b.filename == "loner.epub").unwrap();
        assert_eq!(loner.series, None);
        assert_eq!(loner.series_id, None);
        let loner_uuid = loner.unique_identifier.clone().unwrap();
        let loner_book_id = loner.id;

        // Override the standalone to be part of "Saga". The overrides path
        // does not touch books_series_link, so loner.series_id stays unset
        // in the relational table — get_book must backfill from the series
        // table by name.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &loner_uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, loner_book_id).await.unwrap().unwrap();
        assert_eq!(merged.series.as_deref(), Some("Saga"));
        assert_eq!(
            merged.series_id,
            Some(saga_id),
            "override-only series must still resolve series_id so the detail rail can link"
        );
    }
    #[tokio::test]
    async fn get_book_leaves_series_id_none_when_override_series_unknown() {
        // If the override sets a series name that no other book uses, the
        // series table won't have a row to point at — backfill must
        // leave series_id None rather than fabricating one.
        let _covers = CoversTempDir::new("override_series_unknown");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alone.epub",
                Some("Alone"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let book = &books[0];
        let uuid = book.unique_identifier.clone().unwrap();
        let id = book.id;

        let ov = MetadataOverrides {
            series: Some("A Series That Does Not Yet Exist".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            merged.series.as_deref(),
            Some("A Series That Does Not Yet Exist")
        );
        assert_eq!(merged.series_id, None);
    }
    #[tokio::test]
    async fn list_books_merges_overrides_in_bulk() {
        let _covers = CoversTempDir::new("bulk_merge");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("Book A"), &["Author A"], &[], None, None),
                indexed("b.epub", Some("Book B"), &["Author B"], &[], None, None),
                indexed("c.epub", Some("Book C"), &["Author C"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid_a = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book A"))
            .unwrap()
            .unique_identifier
            .clone()
            .unwrap();
        let uuid_c = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book C"))
            .unwrap()
            .unique_identifier
            .clone()
            .unwrap();

        // Override A and C only.
        upsert_metadata_overrides(
            &pool,
            &uuid_a,
            &MetadataOverrides {
                title: Some("Edited A".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid_c,
            &MetadataOverrides {
                title: Some("Edited C".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books
            .iter()
            .find(|b| b.unique_identifier.as_deref() == Some(&uuid_a))
            .unwrap();
        let b = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book B"))
            .unwrap();
        let c = books
            .iter()
            .find(|b| b.unique_identifier.as_deref() == Some(&uuid_c))
            .unwrap();

        assert_eq!(a.title.as_deref(), Some("Edited A"));
        assert!(a.has_override);
        assert_eq!(b.title.as_deref(), Some("Book B"));
        assert!(!b.has_override);
        assert_eq!(c.title.as_deref(), Some("Edited C"));
        assert!(c.has_override);
    }
    // Additional coverage for core book query functions.
    #[tokio::test]
    async fn list_books_filters_by_library_path() {
        let _covers = CoversTempDir::new("list_books_filter_lib");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed("a1.epub", Some("Alpha One"), &["Author A"], &[], None, None),
                indexed("a2.epub", Some("Alpha Two"), &["Author A"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b1.epub",
                Some("Beta One"),
                &["Author B"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let lib_a = list_books(&pool, "/lib-a").await.unwrap();
        let lib_b = list_books(&pool, "/lib-b").await.unwrap();

        assert_eq!(lib_a.len(), 2, "lib-a should return only its two books");
        let mut titles_a: Vec<String> = lib_a.iter().filter_map(|b| b.title.clone()).collect();
        titles_a.sort();
        assert_eq!(titles_a, vec!["Alpha One", "Alpha Two"]);

        assert_eq!(lib_b.len(), 1, "lib-b should return only its one book");
        assert_eq!(lib_b[0].title.as_deref(), Some("Beta One"));
    }
    #[tokio::test]
    async fn list_books_returns_empty_for_unknown_path() {
        let _covers = CoversTempDir::new("list_books_unknown");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = list_books(&pool, "/does-not-exist").await.unwrap();
        assert!(
            hits.is_empty(),
            "unknown library path should yield an empty vec (no error)"
        );
    }
    #[tokio::test]
    async fn list_books_returns_empty_for_empty_db() {
        let _covers = CoversTempDir::new("list_books_empty_db");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = list_books(&pool, "/lib").await.unwrap();
        assert!(hits.is_empty(), "empty DB should yield an empty vec");
    }
    #[tokio::test]
    async fn search_books_handles_bare_asterisk_without_error() {
        // A raw `*` is an FTS5 operator; the sanitizer must quote it so MATCH
        // doesn't reject the expression as a syntax error. We assert the call
        // succeeds — not a particular hit shape — because the goal is "no panic
        // / no sqlx parse error".
        let _covers = CoversTempDir::new("fts_asterisk");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Anything"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "*")
            .await
            .expect("sanitizer should guard MATCH against the bare `*` operator");
        // `*` alone has no literal token to match; an empty result is fine.
        assert!(hits.is_empty());
    }
    #[tokio::test]
    async fn search_books_handles_bare_double_quote_without_error() {
        // A raw `"` is the FTS5 phrase delimiter. Without sanitization, MATCH
        // would reject this with a parse error and the call would `Err`.
        let _covers = CoversTempDir::new("fts_dquote");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Anything"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "\"")
            .await
            .expect("sanitizer should guard MATCH against a bare `\"` operator");
        assert!(hits.is_empty());
    }
    #[tokio::test]
    async fn search_books_returns_empty_for_unknown_library() {
        // Even with a real match in another library, the WHERE l.path = ?
        // clause must scope results to the requested library.
        let _covers = CoversTempDir::new("search_books_unknown_lib");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Findable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-b", "Findable").await.unwrap();
        assert!(
            hits.is_empty(),
            "query against a non-existent library must not leak rows from another library"
        );
    }
}
