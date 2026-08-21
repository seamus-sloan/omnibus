//! Hardcover provider: the `by_isbn` / `by_title` pair every provider in this
//! directory implements, over the same GraphQL API the suggestions ranking
//! path uses.
//!
//! **Key-gated.** Hardcover authenticates every request, so without a
//! configured key this provider is not a reachable rung at all and the ladder
//! skips it.
//!
//! It sits *last* on the ladder because a lookup here costs two round trips
//! where the catalogs cost one, so it earns its place answering what the big
//! catalogs could not rather than slowing down the common case. In exchange it
//! is the one provider that carries a series statement natively.
//!
//! Its **title search goes through Hardcover's `search` API**, not the
//! `books(where: {title: {_eq: …}})` filter this module used to send. `_ilike`
//! is blocked server-side, so that filter matched only a title typed
//! character-for-character — which meant a query carrying the author (the
//! picker seeds "title author") matched nothing at all, ever. `search` is
//! Hardcover's own full-text endpoint and takes the phrase happily.

use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider, ProviderEdition};
use serde::Deserialize;

use crate::helpers::format_series_index;
use crate::suggestions::hardcover::{post_graphql, HardcoverConfig, HardcoverError};

use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use super::http::{paired_isbn10, sanitize_genres};

/// One query's worth of fields — the book plus a representative edition,
/// nested so a candidate list costs one round trip rather than one per row.
/// Ordering editions by `users_count` mirrors `resolve_book`'s preference for
/// the canonical edition over knockoffs.
///
/// `cached_tags` is Hardcover's denormalized tag bag, whose `Genre` bucket is
/// the closest thing it publishes to a genre list; `pages` and `isbn_10` are
/// edition-level, which is where Hardcover models publication.
const BOOK_FIELDS: &str = "id title description cached_tags \
     contributions { author { name } } \
     book_series { position series { name } } \
     image { url } \
     editions(where: {isbn_13: {_is_null: false}}, order_by: {users_count: desc}, limit: 1) \
       { isbn_13 isbn_10 pages }";

#[derive(Debug, Deserialize)]
struct BooksData {
    #[serde(default)]
    books: Vec<BookRow>,
}

#[derive(Debug, Default, Deserialize)]
struct BookRow {
    /// Hardcover's book id — the handle a selected candidate is re-fetched
    /// by; already requested by [`BOOK_FIELDS`].
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
    /// This book's position in the series, as Hardcover stores it — a float,
    /// because a novella sits at 2.5.
    #[serde(default)]
    position: Option<f64>,
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

/// The `search` response. `results` is an untyped Typesense payload on the
/// wire, so only the parts we read are described — everything else in it
/// (facets, timings, request echo) is ignored by serde rather than modelled.
/// `#[serde(default)]` covers an *absent* key but not a present `null`, and
/// `search` is a nullable GraphQL field — Hasura answers `{"data":{"search":
/// null}}` alongside an `errors` array. `Option` at every level so that reads
/// as "no hits" rather than failing the whole search on a decode error.
#[derive(Debug, Default, Deserialize)]
struct SearchData {
    #[serde(default)]
    search: Option<SearchEnvelope>,
}

#[derive(Debug, Default, Deserialize)]
struct SearchEnvelope {
    #[serde(default)]
    results: Option<SearchResults>,
}

#[derive(Debug, Default, Deserialize)]
struct SearchResults {
    #[serde(default)]
    hits: Option<Vec<SearchHit>>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    #[serde(default)]
    document: Option<SearchDocument>,
}

/// One hit's document. Note the shape differs from [`BookRow`]: a search
/// document carries `author_names` / `featured_series` / `genres` where the
/// `books` query carries `contributions` / `book_series` / `cached_tags`, and
/// its `id` is a **string** where the `books` query's is a number.
#[derive(Debug, Deserialize)]
struct SearchDocument {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author_names: Vec<String>,
    #[serde(default)]
    pages: Option<i64>,
    #[serde(default)]
    release_year: Option<i64>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    image: Option<Image>,
    #[serde(default)]
    featured_series: Option<FeaturedSeries>,
}

#[derive(Debug, Deserialize)]
struct FeaturedSeries {
    #[serde(default)]
    position: Option<f64>,
    #[serde(default)]
    series: Option<Named>,
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

/// POST a query, recording a rate-limit refusal on the way out.
///
/// Hardcover has to notice its own 429 rather than leaning on the shared
/// error-chain sniff in [`super::note_throttle`]: `HardcoverError` wraps the
/// transport failure with `#[error(transparent)]`, whose generated `source()`
/// forwards *past* the `reqwest::Error` to that error's own source — so the
/// status never appears in the chain and a downcast finds nothing. Pinned by
/// `hardcover_429_records_a_cooldown`.
async fn post<T: serde::de::DeserializeOwned>(
    config: &MetadataLookupConfig,
    hc: &HardcoverConfig,
    query: &str,
    variables: serde_json::Value,
) -> anyhow::Result<T> {
    match post_graphql(hc, query, variables).await {
        Ok(found) => Ok(found),
        Err(e) => {
            if let HardcoverError::Http(ref transport) = e {
                if transport.status() == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) {
                    config.throttle.record(MetadataProvider::Hardcover, None);
                }
            }
            Err(e.into())
        }
    }
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
        post(config, &hc, query, serde_json::json!({ "isbn": isbn13 })).await?;
    let Some(book_id) = data.editions.into_iter().find_map(|e| e.book_id) else {
        return Ok(None);
    };

    let query = format!(
        "query ($id: Int!) {{ books(where: {{id: {{_eq: $id}}}}, limit: 1) {{ {BOOK_FIELDS} }} }}"
    );
    let data: BooksData = post(config, &hc, &query, serde_json::json!({ "id": book_id })).await?;
    // The scanned barcode is authoritative here, exactly as on the other
    // providers' ISBN path — Hardcover resolves to a *work*, whose
    // representative edition is very often a different printing.
    Ok(data
        .books
        .into_iter()
        .next()
        .and_then(|b| map_book(b, Some(isbn13))))
}

/// Re-fetch one candidate by its Hardcover book id — the hydrate path for a
/// candidate with no ISBN, which is every candidate the `search` endpoint
/// returns.
///
/// `Ok(None)` when the handle isn't a book id (an `isbn:` fallback ref belongs
/// on the ISBN path), no key is configured, or Hardcover no longer has the
/// book. The id rides in `variables` as an `Int`, so it is parsed rather than
/// interpolated.
pub async fn by_ref(
    config: &MetadataLookupConfig,
    provider_ref: &str,
) -> anyhow::Result<Option<ProviderEdition>> {
    let Some(hc) = client_config(config) else {
        return Ok(None);
    };
    // `$id: Int!` is 32-bit in GraphQL, so a wider value would pass a local
    // parse and then be refused by the server as a coercion error — which
    // hydrate would surface as an outage rather than the miss it is.
    let Ok(book_id) = provider_ref.trim().parse::<i32>() else {
        return Ok(None);
    };
    let query = format!(
        "query ($id: Int!) {{ books(where: {{id: {{_eq: $id}}}}, limit: 1) {{ {BOOK_FIELDS} }} }}"
    );
    let data: BooksData = post(config, &hc, &query, serde_json::json!({ "id": book_id })).await?;
    Ok(data
        .books
        .into_iter()
        .next()
        .and_then(|b| map_book(b, None)))
}

/// Search by free text through Hardcover's own `search` endpoint.
///
/// `Ok(empty)` when nothing matches or no key is configured.
///
/// The response is a Typesense payload behind an untyped `results` scalar, so
/// it is read defensively: a hit whose document has no title or no id is
/// skipped rather than failing the search.
///
/// **A search document describes a *work*, not a printing.** Its `isbns`
/// array spans every edition Hardcover knows, so there is no ISBN that can
/// honestly be attributed to the candidate — [`ProviderEdition::isbn13`] is
/// left `None` and the picker's hydrate step resolves the edition on select.
/// Inventing one by taking `isbns[0]` would put a specific printing's
/// identifier on a row describing all of them.
pub async fn by_text(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ProviderEdition>> {
    let Some(hc) = client_config(config) else {
        return Ok(Vec::new());
    };
    // Untrusted text rides in `variables`, never interpolated into the query.
    let gql = "query ($q: String!, $limit: Int!) { \
         search(query: $q, query_type: \"Book\", per_page: $limit, page: 1) { results } }";
    let data: SearchData = post(
        config,
        &hc,
        gql,
        serde_json::json!({ "q": query, "limit": SEARCH_LIMIT }),
    )
    .await?;
    Ok(data
        .search
        .and_then(|s| s.results)
        .and_then(|r| r.hits)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|hit| map_search_document(hit.document?))
        .collect())
}

/// Map one `search` hit document into a candidate. `None` when it carries no
/// title or no id — without an id there is no handle to hydrate it by.
fn map_search_document(doc: SearchDocument) -> Option<ProviderEdition> {
    let title = doc.title.filter(|t| !t.trim().is_empty())?;
    let provider_ref = doc.id.filter(|id| !id.trim().is_empty())?;
    let series_row = doc.featured_series.and_then(|fs| {
        let name = fs
            .series
            .and_then(|s| s.name)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= ExternalBookMeta::NAME_MAX_LEN)?;
        Some((name, fs.position.map(format_series_index)))
    });
    Some(ProviderEdition {
        source: MetadataProvider::Hardcover,
        provider_ref,
        // See the note on `by_text`: `isbns` spans every edition of the work.
        isbn13: None,
        isbn10: None,
        title,
        authors: doc.author_names,
        // `release_year` is the work's, not an edition's — which is exactly
        // what `first_publish_year` means, and not what `year` means.
        year: None,
        pages: doc.pages.filter(|p| *p > 0),
        publisher: None,
        description: doc
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        cover_url: doc.image.and_then(|i| i.url),
        series: series_row.as_ref().map(|(name, _)| name.clone()),
        series_index: series_row.and_then(|(_, index)| index),
        first_publish_year: doc.release_year,
        genres: sanitize_genres(doc.genres),
        relevance: None,
    })
}

/// Read the `Genre` bucket out of Hardcover's `cached_tags` blob.
///
/// The column is jsonb and its buckets are data, so this reads defensively
/// rather than deriving a type. The live shape is
/// `{"Tag": [...], "Mood": [...], "Genre": [{"tag": "Programming", "count": 1,
/// "tagSlug": …}], "Content Warning": [...]}`; a bare array of tags (or of
/// strings) is accepted too, and anything else yields no genres instead of an
/// error — a tag bag is never worth failing a lookup over.
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

/// The first usable `(series name, book number)` pair, read off one row.
///
/// Returned as a pair rather than two lookups because a book can belong to
/// more than one series: taking the name from whichever row happens to have
/// one and the position from whichever happens to have that would report a
/// number from a series the book isn't being filed under.
fn series_statement(rows: Vec<BookSeries>) -> Option<(String, Option<String>)> {
    rows.into_iter().find_map(|bs| {
        let name = bs
            .series
            .and_then(|s| s.name)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= ExternalBookMeta::NAME_MAX_LEN)?;
        Some((name, bs.position.map(format_series_index)))
    })
}

/// Map a book row into `ProviderEdition`. `isbn13` overrides the row's own
/// when the caller has an authoritative one (the ISBN path). A row with no
/// title, or no handle to re-fetch it by, maps to `None`.
fn map_book(b: BookRow, isbn13: Option<&str>) -> Option<ProviderEdition> {
    let title = b.title.filter(|t| !t.trim().is_empty())?;
    let book_id = b.id;
    let series_row = series_statement(b.book_series);
    // At most one row: the nested selection asks for `limit: 1`.
    let edition = b.editions.into_iter().next().unwrap_or_default();
    let edition_isbn13 = edition
        .isbn_13
        .as_deref()
        .and_then(|v| normalize_isbn(v).ok());
    let isbn13 = match isbn13 {
        Some(scanned) => Some(scanned.to_string()),
        None => edition_isbn13.clone(),
    };
    // The book id is the handle; without one there is nothing to re-fetch by,
    // and an `isbn:` handle is only a handle when there is an ISBN.
    let provider_ref = match (book_id, isbn13.as_deref()) {
        (Some(id), _) => id.to_string(),
        (None, Some(isbn)) => format!("isbn:{isbn}"),
        (None, None) => return None,
    };
    // Hardcover's `pages` is per *edition*, and on the ISBN path the scanned
    // barcode — not the most-read printing this row carries — is what we are
    // describing. Reporting that printing's length for a different one is the
    // wrong-value-that-looks-right this guards; `isbn10` is held to the same
    // rule by `paired_isbn10`.
    // Both `None` means this row carried no edition at all, in which case
    // `edition` is the default and there are no pages to mis-attribute.
    let pages = if edition_isbn13 == isbn13 {
        edition.pages.filter(|p| *p > 0)
    } else {
        None
    };
    Some(ProviderEdition {
        source: MetadataProvider::Hardcover,
        provider_ref,
        // An ISBN-10 is a pairing with an ISBN-13; with no ISBN-13 to pair to,
        // there is nothing to assert.
        isbn10: isbn13
            .as_deref()
            .and_then(|i13| paired_isbn10(edition.isbn_10.as_deref(), i13)),
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
        // Name and position are read off the *same* row — a book can belong
        // to more than one series, and pairing one series' name with
        // another's position would be worse than reporting neither.
        series: series_row.as_ref().map(|(name, _)| name.clone()),
        series_index: series_row.and_then(|(_, index)| index),
        first_publish_year: None,
        genres: sanitize_genres(genre_tags(b.cached_tags)),
        relevance: None,
    })
}

#[cfg(test)]
mod tests;
