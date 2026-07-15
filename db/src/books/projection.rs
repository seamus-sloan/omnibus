//! Shared projection helpers: column lists, JSON row decoders, description
//! sanitization, the `BOOK_COLUMNS -> EbookMetadata` mapper, and the post-
//! merge `Contributor::id` backfill. Used by every read path in
//! `crate::books`, plus the discovery read paths (`get_author`,
//! `get_series`) via `pub(crate)` re-exports.

use omnibus_shared::{Contributor, EbookMetadata, Identifier};
use sqlx::{Row, SqlitePool};

use crate::helpers::format_series_index;
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk};

/// Hard server-side cap on the number of books any single list/search
/// response returns. 50k is well above the client-side sort/filter
/// ceiling (anything beyond needs server-side pagination anyway) and
/// small enough that JSON-encoding the response stays in a sensible
/// memory envelope.
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
///
/// Single-valued joins that need two columns from the *same* picked row
/// (the primary `book_files` row, the primary `books_series_link` row) are
/// pulled as one `json_object` subquery rather than two correlated scalar
/// subqueries scanning the same table twice — mirroring the `json_object`
/// shape already used for creators/identifiers. `row_to_ebook` decodes
/// these blobs.
pub(crate) const BOOK_COLUMNS: &str = r"
    b.id, b.uuid,
    b.title, b.description, b.series_index, b.has_cover,
    b.pubdate,
    -- `last_modified`/`timestamp` are INTEGER unix-seconds (migration 0038);
    -- format back to fixed-width ISO so the wire `EbookMetadata.modified` /
    -- `added_at` stay `Option<String>` and the landing lexicographic sort
    -- keeps working (ISO sorts identically to chronological).
    strftime('%Y-%m-%dT%H:%M:%SZ', b.last_modified, 'unixepoch') AS last_modified,
    strftime('%Y-%m-%dT%H:%M:%SZ', b.timestamp,     'unixepoch') AS timestamp,
    b.accent_color,

    (SELECT json_object('filename', bf.filename, 'format', bf.format)
       FROM book_files bf
      WHERE bf.book_id = b.id
      ORDER BY (bf.format != 'EPUB'), bf.format
      LIMIT 1)                                   AS primary_file_json,

    (SELECT pub.name FROM books_publishers_link bpl
       JOIN publishers pub ON pub.id = bpl.publisher
      WHERE bpl.book = b.id ORDER BY pub.name LIMIT 1)
                                                  AS publisher_name,

    (SELECT lang.code FROM books_languages_link bll
       JOIN languages lang ON lang.id = bll.language
      WHERE bll.book = b.id ORDER BY lang.code LIMIT 1)
                                                  AS language_code,

    (SELECT json_object('name', s.name, 'id', s.id)
       FROM books_series_link bsl
       JOIN series s ON s.id = bsl.series
      WHERE bsl.book = b.id ORDER BY s.name LIMIT 1)
                                                  AS series_json,

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
       FROM (SELECT DISTINCT format FROM book_files
              WHERE book_id = b.id
              ORDER BY format))                   AS formats_json
";

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

/// Decoded `primary_file_json` blob: the filename stem + format of the
/// primary `book_files` row (EPUB-preferred). Both columns come from the
/// *same* picked row, so a single subquery replaces the former pair.
#[derive(serde::Deserialize)]
pub(crate) struct PrimaryFileRow {
    pub(crate) filename: String,
    pub(crate) format: String,
}

/// Decoded `series_json` blob: the name + id of the primary
/// `books_series_link` row (alphabetical by name). Both columns come from
/// the *same* picked row, so a single subquery replaces the former pair.
#[derive(serde::Deserialize)]
pub(crate) struct SeriesRow {
    pub(crate) name: String,
    pub(crate) id: i64,
}

/// Decode a single `json_object` blob produced by a `LIMIT 1` subquery.
/// `None` (SQLite NULL — the subquery matched no row) maps to `Ok(None)`,
/// so callers tolerate the "no primary file" / "no series" case exactly as
/// the former two-NULL-column pair did.
pub(crate) fn parse_json_object<T: serde::de::DeserializeOwned>(
    blob: Option<String>,
) -> Result<Option<T>, sqlx::Error> {
    match blob {
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| sqlx::Error::Decode(Box::new(e))),
        None => Ok(None),
    }
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

/// Derive the book's primary ISBN-13 from its scanned `book_identifiers`
/// rows: the first identifier whose scheme mentions "isbn" (case-
/// insensitive — OPF scheme text is free-form) and whose value strips down
/// to exactly 13 ASCII digits. `None` when no identifier matches; the
/// metadata-edit override (`apply_overrides`) is then the only source.
pub(crate) fn derive_isbn13(identifiers: &[Identifier]) -> Option<String> {
    identifiers.iter().find_map(|ident| {
        let is_isbn_scheme = ident
            .scheme
            .as_deref()
            .is_some_and(|s| s.to_ascii_lowercase().contains("isbn"));
        if !is_isbn_scheme {
            return None;
        }
        let digits: String = ident.value.chars().filter(char::is_ascii_digit).collect();
        (digits.chars().count() == 13).then_some(digits)
    })
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
    let primary_file = parse_json_object::<PrimaryFileRow>(r.get("primary_file_json"))?;
    let filename = match primary_file {
        Some(pf) => format!("{}.{}", pf.filename, pf.format.to_ascii_lowercase()),
        None => String::new(),
    };
    let series = parse_json_object::<SeriesRow>(r.get("series_json"))?;
    let (series_name, series_link_id) = match series {
        Some(s) => (Some(s.name), Some(s.id)),
        None => (None, None),
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
    let isbn13 = derive_isbn13(&identifiers);

    Ok(EbookMetadata {
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
        isbn13,
        series: series_name,
        series_index: series_index.map(format_series_index),
        series_id: series_link_id,
        unique_identifier: Some(uuid.clone()),
        cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
        accent: r.get("accent_color"),
        formats: parse_json_array(r.get("formats_json"))?,
        added_at: r.get("timestamp"),
        error: None,
        has_override: false,
        has_cover_override: false,
        book_files: Vec::new(),
        // Populated by `get_book` from the resolved EPUB; list/projection rows
        // don't carry it (no per-book export menu in list contexts).
        epub_size_bytes: None,
    })
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

/// Bulk-merge user-supplied `metadata_overrides` into every book in `books` in place.
pub(crate) async fn merge_overrides_into_books(
    pool: &SqlitePool,
    books: &mut [EbookMetadata],
) -> Result<(), sqlx::Error> {
    let uuids: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for book in books.iter_mut() {
        // Snapshot uuid first so the overrides_map lookup is independent
        // of the `&mut book` passed into apply_overrides.
        let uuid_owned = book.unique_identifier.clone();
        if let Some(uuid) = uuid_owned.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, uuid, ov, *has_cover_ov);
            }
        }
    }
    Ok(())
}
