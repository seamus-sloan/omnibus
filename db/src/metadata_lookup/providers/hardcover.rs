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
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use serde::Deserialize;

use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use crate::suggestions::hardcover::{post_graphql, HardcoverConfig};

/// One query's worth of fields — the book plus a representative edition's
/// ISBN-13, nested so a candidate list costs one round trip rather than one
/// per row. Ordering editions by `users_count` mirrors `resolve_book`'s
/// preference for the canonical edition over knockoffs.
const BOOK_FIELDS: &str = "id title description \
     contributions { author { name } } \
     book_series { series { name } } \
     image { url } \
     editions(where: {isbn_13: {_is_null: false}}, order_by: {users_count: desc}, limit: 1) \
       { isbn_13 }";

#[derive(Debug, Deserialize)]
struct BooksData {
    #[serde(default)]
    books: Vec<BookRow>,
}

#[derive(Debug, Deserialize)]
struct BookRow {
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

#[derive(Debug, Deserialize)]
struct Edition {
    #[serde(default)]
    isbn_13: Option<String>,
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
) -> anyhow::Result<Option<ExternalBookMeta>> {
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

    let query = format!(
        "query ($id: Int!) {{ books(where: {{id: {{_eq: $id}}}}, limit: 1) {{ {BOOK_FIELDS} }} }}"
    );
    let data: BooksData = post_graphql(&hc, &query, serde_json::json!({ "id": book_id })).await?;
    // The scanned barcode is authoritative here, exactly as on the other
    // providers' ISBN path — Hardcover resolves to a *work*, whose
    // representative edition is very often a different printing.
    Ok(data
        .books
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
) -> anyhow::Result<Vec<ExternalBookMeta>> {
    let Some(hc) = client_config(config) else {
        return Ok(Vec::new());
    };
    let gql = format!(
        "query ($title: String!, $limit: Int!) {{ \
           books(where: {{title: {{_eq: $title}}}}, order_by: {{users_count: desc}}, limit: $limit) \
           {{ {BOOK_FIELDS} }} }}"
    );
    let data: BooksData = post_graphql(
        &hc,
        &gql,
        serde_json::json!({ "title": query, "limit": SEARCH_LIMIT }),
    )
    .await?;
    Ok(data
        .books
        .into_iter()
        .filter_map(|b| map_book(b, None))
        .collect())
}

/// Map a book row into `ExternalBookMeta`. `isbn13` overrides the row's own
/// when the caller has an authoritative one (the ISBN path). A row with no
/// title, or no ISBN to fall back on, maps to `None` — the check-in flow keys
/// on the ISBN downstream, so a candidate without one isn't actionable.
fn map_book(b: BookRow, isbn13: Option<&str>) -> Option<ExternalBookMeta> {
    let title = b.title.filter(|t| !t.trim().is_empty())?;
    let isbn13 = match isbn13 {
        Some(scanned) => scanned.to_string(),
        None => b
            .editions
            .into_iter()
            .find_map(|e| e.isbn_13.and_then(|v| normalize_isbn(&v).ok()))?,
    };
    Some(ExternalBookMeta {
        isbn13,
        title,
        authors: b
            .contributions
            .into_iter()
            .filter_map(|c| c.author.and_then(|a| a.name))
            .collect(),
        // Hardcover models publication on the edition, not the work, and this
        // ladder only reads the work — so no year, pages, or publisher.
        year: None,
        pages: None,
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
        source: MetadataProvider::Hardcover,
    })
}

#[cfg(test)]
mod tests;
