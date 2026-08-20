//! Hardcover provider: the `by_isbn` / `by_title` pair every provider in this
//! directory implements, over the same GraphQL API the suggestions ranking
//! path uses.
//!
//! **Key-gated.** Hardcover authenticates every request, so without a
//! configured key this provider is not a reachable rung at all and the ladder
//! skips it.
//!
//! It sits *last* on the ladder for two reasons: a lookup here costs two
//! round trips where the catalogs cost one, and its title search is exact-match
//! only (`_ilike` is blocked server-side), so it earns its place as the rung
//! that answers when the big catalogs have already come up empty — not as one
//! that slows down the common case. In exchange it is the one provider that
//! carries a series statement natively.

use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider, ProviderEdition};
use serde::Deserialize;

use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use super::http::{paired_isbn10, sanitize_genres};
use crate::suggestions::hardcover::{post_graphql, HardcoverConfig, HardcoverError};

/// One query's worth of fields — the book plus a representative edition,
/// nested so a candidate list costs one round trip rather than one per row.
/// Ordering editions by `users_count` mirrors `resolve_book`'s preference for
/// the canonical edition over knockoffs.
///
/// `enriched` adds the fields only the picker wants: `cached_tags` (whose
/// `Genre` bucket is the closest thing Hardcover publishes to a genre list)
/// and the edition's `isbn_10` / `pages`. They are separable because
/// [`fetch_books`] falls back to the base selection when Hardcover rejects
/// the query — see there for why.
fn book_fields(enriched: bool) -> String {
    let (book_extra, edition_extra) = if enriched {
        (" cached_tags", " isbn_10 pages")
    } else {
        ("", "")
    };
    format!(
        "id title description{book_extra} \
         contributions {{ author {{ name }} }} \
         book_series {{ series {{ name }} }} \
         image {{ url }} \
         editions(where: {{isbn_13: {{_is_null: false}}}}, order_by: {{users_count: desc}}, limit: 1) \
           {{ isbn_13{edition_extra} }}"
    )
}

#[derive(Debug, Deserialize)]
struct BooksData {
    #[serde(default)]
    books: Vec<BookRow>,
}

#[derive(Debug, Default, Deserialize)]
struct BookRow {
    /// Hardcover's book id — the handle a selected candidate is re-fetched
    /// by; already requested by [`book_fields`].
    #[serde(default)]
    id: Option<i64>,
    /// Hardcover's denormalized tag bag, kept untyped: it is a jsonb column
    /// whose buckets (`Genre`, `Mood`, `Tag`, …) are data rather than schema.
    /// [`genre_tags`] does the reading.
    #[serde(default)]
    cached_tags: Option<serde_json::Value>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    contributions: Vec<Contribution>,
    #[serde(default)]
    book_series: Vec<BookSeries>,
    #[serde(default)]
    image: Option<Image>,
    #[serde(default)]
    editions: Vec<Edition>,
}

#[derive(Debug, Deserialize)]
struct Contribution {
    #[serde(default)]
    author: Option<Named>,
}

#[derive(Debug, Deserialize)]
struct Named {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BookSeries {
    #[serde(default)]
    series: Option<Named>,
}

#[derive(Debug, Deserialize)]
struct Image {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Edition {
    #[serde(default)]
    isbn_13: Option<String>,
    #[serde(default)]
    isbn_10: Option<String>,
    /// Printed length. Hardcover models publication on the edition, not the
    /// work, so this is the only place a page count exists here.
    #[serde(default)]
    pages: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct EditionsData {
    #[serde(default)]
    editions: Vec<EditionBookId>,
}

#[derive(Debug, Deserialize)]
struct EditionBookId {
    #[serde(default)]
    book_id: Option<i64>,
}

/// Build the suggestions client's config from the ladder's, so the endpoint is
/// injectable for tests and — importantly — the request inherits the ladder's
/// short timeout rather than the suggestions path's 30s one. A rung that hung
/// for half a minute would strand the scan flow on a spinner.
fn client_config(config: &MetadataLookupConfig) -> Option<HardcoverConfig> {
    Some(HardcoverConfig {
        base_url: config.hardcover_base.clone(),
        api_key: config.keys.hardcover.clone()?,
        timeout: config.timeout,
    })
}

/// Look up an ISBN: resolve its edition to a `book_id`, then hydrate the book.
/// `Ok(None)` when Hardcover doesn't know the ISBN — or when no key is
/// configured, which is a miss rather than an error so an unconfigured
/// instance is indistinguishable from one Hardcover simply couldn't help.
pub async fn by_isbn(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> anyhow::Result<Option<ProviderEdition>> {
    let Some(hc) = client_config(config) else {
        return Ok(None);
    };
    // Untrusted values ride in `variables`, never interpolated into the query.
    let query = "query ($isbn: String!) { \
         editions(where: {_or: [{isbn_13: {_eq: $isbn}}, {isbn_10: {_eq: $isbn}}]}, limit: 1) \
         { book_id } }";
    let data: EditionsData =
        post_graphql(&hc, query, serde_json::json!({ "isbn": isbn13 })).await?;
    let Some(book_id) = data.editions.into_iter().find_map(|e| e.book_id) else {
        return Ok(None);
    };

    let books = fetch_books(
        &hc,
        |fields| {
            format!(
                "query ($id: Int!) {{ books(where: {{id: {{_eq: $id}}}}, limit: 1) {{ {fields} }} }}"
            )
        },
        serde_json::json!({ "id": book_id }),
    )
    .await?;
    // The scanned barcode is authoritative here, exactly as on the other
    // providers' ISBN path — Hardcover resolves to a *work*, whose
    // representative edition is very often a different printing.
    Ok(books
        .into_iter()
        .next()
        .and_then(|b| map_book(b, Some(isbn13))))
}

/// Search by exact title. `_ilike` is blocked server-side, so this only
/// answers when the reader typed the title exactly; `users_count desc` floats
/// the canonical edition above summary/knockoff entries. `Ok(empty)` when
/// nothing matches or no key is configured.
pub async fn by_title(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ProviderEdition>> {
    let Some(hc) = client_config(config) else {
        return Ok(Vec::new());
    };
    let books = fetch_books(
        &hc,
        |fields| {
            format!(
                "query ($title: String!, $limit: Int!) {{ \
                   books(where: {{title: {{_eq: $title}}}}, order_by: {{users_count: desc}}, limit: $limit) \
                   {{ {fields} }} }}"
            )
        },
        serde_json::json!({ "title": query, "limit": SEARCH_LIMIT }),
    )
    .await?;
    Ok(books
        .into_iter()
        .filter_map(|b| map_book(b, None))
        .collect())
}

/// Run a `books` query with the enriched selection, falling back to the base
/// one if Hardcover rejects it.
///
/// A `HardcoverError::Graphql` on a 200 is Hasura refusing the *query* — which
/// is what an unknown field in the selection set looks like, and the enriched
/// fields are the ones this instance cannot verify against a live schema. The
/// retry keeps a schema drift costing the three new fields rather than the
/// whole provider: without it, a renamed `cached_tags` would take every
/// Hardcover lookup down with it, including the check-in ladder's. Transport
/// failures are not retried — they say nothing about the selection.
async fn fetch_books(
    hc: &HardcoverConfig,
    query_for: impl Fn(&str) -> String,
    variables: serde_json::Value,
) -> anyhow::Result<Vec<BookRow>> {
    match post_graphql::<BooksData>(hc, &query_for(&book_fields(true)), variables.clone()).await {
        Ok(data) => Ok(data.books),
        Err(HardcoverError::Graphql(msg)) => {
            tracing::warn!(
                "hardcover rejected the enriched selection ({msg}); retrying without it"
            );
            let data: BooksData =
                post_graphql(hc, &query_for(&book_fields(false)), variables).await?;
            Ok(data.books)
        }
        Err(e) => Err(e.into()),
    }
}

/// Read the `Genre` bucket out of Hardcover's `cached_tags` blob.
///
/// The column is jsonb and its buckets are data, so this reads defensively
/// rather than deriving a type: the observed shape is
/// `{"Genre": [{"tag": "Fantasy", …}], "Mood": [...]}`, and a bare array of
/// tags (or of strings) is accepted too. Anything else yields no genres
/// instead of an error — a tag bag is never worth failing a lookup over.
fn genre_tags(cached: Option<serde_json::Value>) -> Vec<String> {
    let bucket = match cached {
        Some(serde_json::Value::Object(mut map)) => map.remove("Genre").unwrap_or_default(),
        Some(array @ serde_json::Value::Array(_)) => array,
        _ => return Vec::new(),
    };
    let serde_json::Value::Array(entries) = bucket else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            serde_json::Value::String(tag) => Some(tag),
            serde_json::Value::Object(mut fields) => match fields.remove("tag") {
                Some(serde_json::Value::String(tag)) => Some(tag),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Map a book row into `ProviderEdition`. `isbn13` overrides the row's own
/// when the caller has an authoritative one (the ISBN path). A row with no
/// title, or no ISBN to fall back on, maps to `None` — the check-in flow keys
/// on the ISBN downstream, so a candidate without one isn't actionable.
fn map_book(b: BookRow, isbn13: Option<&str>) -> Option<ProviderEdition> {
    let title = b.title.filter(|t| !t.trim().is_empty())?;
    let book_id = b.id;
    // At most one row: the nested selection asks for `limit: 1`.
    let edition = b.editions.into_iter().next().unwrap_or_default();
    let edition_isbn13 = edition
        .isbn_13
        .as_deref()
        .and_then(|v| normalize_isbn(v).ok());
    let isbn13 = match isbn13 {
        Some(scanned) => scanned.to_string(),
        None => edition_isbn13.clone()?,
    };
    // Hardcover's `pages` is per *edition*, and on the ISBN path the scanned
    // barcode — not the most-read printing this row carries — is what we are
    // describing. Reporting that printing's length for a different one is the
    // wrong-value-that-looks-right this guards; `isbn10` is held to the same
    // rule by `paired_isbn10`.
    let pages = if edition_isbn13.as_deref() == Some(isbn13.as_str()) {
        edition.pages.filter(|p| *p > 0)
    } else {
        None
    };
    Some(ProviderEdition {
        source: MetadataProvider::Hardcover,
        provider_ref: book_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| format!("isbn:{isbn13}")),
        isbn10: paired_isbn10(edition.isbn_10.as_deref(), &isbn13),
        isbn13,
        title,
        authors: b
            .contributions
            .into_iter()
            .filter_map(|c| c.author.and_then(|a| a.name))
            .collect(),
        // Hardcover models publication on the edition, not the work, and the
        // only edition field this ladder reads is the representative one's —
        // so no year or publisher.
        year: None,
        pages,
        publisher: None,
        description: b
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        cover_url: b.image.and_then(|i| i.url),
        // The reason this provider is worth a rung: the only one that answers
        // with a series statement rather than needing the enrichment lookup.
        series: b
            .book_series
            .into_iter()
            .find_map(|bs| bs.series.and_then(|s| s.name))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= ExternalBookMeta::NAME_MAX_LEN),
        first_publish_year: None,
        genres: sanitize_genres(genre_tags(b.cached_tags)),
    })
}

#[cfg(test)]
mod tests;
