//! Hardcover GraphQL client for suggestions. Hardcover has no
//! recommendations endpoint, so we resolve the library book to a Hardcover
//! book, collect the curated lists it sits on, and rank every other book by
//! how many of those lists it shares. All queries go through [`post_graphql`]
//! against [`HardcoverConfig`] (base URL + Bearer key + timeout).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use futures::future::join_all;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::suggestions::filter::Candidate;

/// Process-wide `reqwest::Client` for Hardcover GraphQL calls, built once and
/// cloned so resolutions share one connection pool + TLS session cache.
/// Fallible (TLS backend init); callers propagate via `?`.
fn hardcover_client() -> reqwest::Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let new = crate::http_client::build_client(&crate::http_client::default_user_agent())?;
    Ok(CLIENT.get_or_init(|| new).clone())
}

/// Hardcover's per-query row cap (verified live: a `limit: 5000` returns 2000).
const PAGE_SIZE: i64 = 2000;
/// How many curated lists to sample for one book. The book may sit on
/// thousands; richer lists (nearer 150) carry the most co-occurrence overlap,
/// so we take the largest curated ones first and bound the work.
const LIST_SAMPLE: i64 = 40;
/// Hard ceiling on co-listing member rows pulled across all sampled lists.
const MAX_MEMBER_ROWS: usize = 6000;
/// Curated-list size band: below 5 is noise, above 150 is a dumping ground.
const MIN_LIST_BOOKS: i64 = 5;
const MAX_LIST_BOOKS: i64 = 150;
/// Public list privacy flag (Hardcover `privacy_setting_id`).
const PRIVACY_PUBLIC: i64 = 1;
/// Depth of the candidate pool fetched for detail+filtering before trimming to
/// the final 10 — taken deep because the same-author/same-series filter prunes
/// hard (an author-heavy book sheds its whole top-of-list).
pub const CANDIDATE_POOL: usize = 70;
/// Per-request timeout. Hardcover allows up to 30s per query.
const HARDCOVER_TIMEOUT: Duration = Duration::from_secs(30);

/// Connection config for the Hardcover GraphQL API. Injectable so tests point
/// `base_url` at a local `wiremock` server.
#[derive(Debug, Clone)]
pub struct HardcoverConfig {
    /// `https://api.hardcover.app/v1/graphql` in production.
    pub base_url: String,
    /// Account-level Bearer token (server-wide, never per-user).
    pub api_key: String,
    pub timeout: Duration,
}

impl HardcoverConfig {
    /// Build a config for the live endpoint with the given key.
    pub fn new(api_key: String) -> Self {
        Self {
            base_url: "https://api.hardcover.app/v1/graphql".to_string(),
            api_key,
            timeout: HARDCOVER_TIMEOUT,
        }
    }
}

/// Errors from the Hardcover client. `Graphql` carries the joined `errors[]`
/// messages from a 200 response that nonetheless failed (Hasura convention).
#[derive(Debug, thiserror::Error)]
pub enum HardcoverError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("hardcover graphql error: {0}")]
    Graphql(String),
}

/// The Hardcover book a library book resolved to, plus the fields the filter
/// needs to exclude same-author / same-series candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBook {
    pub id: i64,
    pub author_names: Vec<String>,
    pub series: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphqlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

/// POST a GraphQL operation and deserialize `data` into `T`. Returns
/// [`HardcoverError::Graphql`] when the response carries `errors[]` or no
/// `data`. Integer-id arrays are inlined by callers (safe — `i64`); all
/// untrusted strings go through `variables`.
async fn post_graphql<T: DeserializeOwned>(
    config: &HardcoverConfig,
    query: &str,
    variables: serde_json::Value,
) -> Result<T, HardcoverError> {
    let client = hardcover_client()?;
    let resp = client
        .post(&config.base_url)
        .timeout(config.timeout)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", config.api_key),
        )
        .json(&serde_json::json!({ "query": query, "variables": variables }))
        .send()
        .await?
        .error_for_status()?;
    let envelope: GraphqlEnvelope<T> = resp.json().await?;
    if !envelope.errors.is_empty() {
        let joined = envelope
            .errors
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(HardcoverError::Graphql(joined));
    }
    envelope
        .data
        .ok_or_else(|| HardcoverError::Graphql("response had no data".to_string()))
}

// ── Book resolution ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct EditionsData {
    editions: Vec<EditionRow>,
}
#[derive(Debug, Deserialize)]
struct EditionRow {
    book_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BooksData {
    books: Vec<BookRow>,
}
#[derive(Debug, Deserialize)]
struct BookRow {
    id: i64,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    contributions: Vec<Contribution>,
    #[serde(default)]
    book_series: Vec<BookSeries>,
    #[serde(default)]
    image: Option<HcImage>,
    /// Only populated when the query appends `description` to
    /// [`BOOK_FIELDS`] (see [`fetch_book_details`]) — absent from the
    /// response JSON on every other query, which `#[serde(default)]`
    /// turns into `None` rather than a deserialization error.
    #[serde(default)]
    description: Option<String>,
}
#[derive(Debug, Deserialize)]
struct Contribution {
    #[serde(default)]
    author: Option<HcAuthor>,
}
#[derive(Debug, Deserialize)]
struct HcAuthor {
    #[serde(default)]
    name: Option<String>,
}
#[derive(Debug, Deserialize)]
struct BookSeries {
    #[serde(default)]
    position: Option<f64>,
    #[serde(default)]
    series: Option<HcSeries>,
}
#[derive(Debug, Deserialize)]
struct HcSeries {
    #[serde(default)]
    name: Option<String>,
}
#[derive(Debug, Deserialize)]
struct HcImage {
    #[serde(default)]
    url: Option<String>,
}

const BOOK_FIELDS: &str = "id slug title contributions { author { name } } \
     book_series { position series { name } } image { url }";

/// Max concurrent per-ISBN Hardcover lookups in [`resolve_book`].
const ISBN_RESOLVE_CONCURRENCY: usize = 4;

/// Resolve a library book to a Hardcover book. ISBNs are tried first (most
/// reliable); on a miss we fall back to an **exact** title match ordered by
/// `users_count` so the canonical edition wins over summary/knockoff entries
/// (which carry near-zero reads). Returns `None` when nothing resolves.
pub async fn resolve_book(
    config: &HardcoverConfig,
    isbns: &[String],
    title: &str,
    author: Option<&str>,
) -> Result<Option<ResolvedBook>, HardcoverError> {
    // Look up every ISBN with bounded concurrency (at most
    // `ISBN_RESOLVE_CONCURRENCY` in flight at once, chunk by chunk — plain
    // `join_all` per chunk, not `futures::stream::{buffered,
    // buffer_unordered}`, which trip a rustc higher-ranked-lifetime
    // auto-trait inference bug elsewhere in this crate: a spurious "`Send`
    // is not general enough" error on the unrelated `Worker::run` dispatch
    // in `worker/exec.rs`, confirmed by bisecting — the same error surfaces
    // regardless of which call site introduces the `Stream` combinator or
    // an `AsyncFn`-bound generic helper) but still decide the winner exactly
    // as the old sequential loop would: walk the results in order and stop
    // at the first ISBN that either errors or resolves, so an ISBN's
    // priority over a later one is unaffected by fetching concurrently.
    let mut by_isbn: Vec<Result<Option<ResolvedBook>, HardcoverError>> =
        Vec::with_capacity(isbns.len());
    for chunk in isbns.chunks(ISBN_RESOLVE_CONCURRENCY) {
        by_isbn.extend(join_all(chunk.iter().map(|isbn| resolve_by_isbn(config, isbn))).await);
    }
    for result in by_isbn {
        match result? {
            Some(resolved) => return Ok(Some(resolved)),
            None => continue,
        }
    }

    // Title fallback. `_ilike` is blocked server-side, so match exact title and
    // let `users_count desc` float the canonical edition above knockoffs.
    let query = format!(
        "query ($title: String!) {{ books(where: {{title: {{_eq: $title}}}}, \
         order_by: {{users_count: desc}}, limit: 5) {{ {BOOK_FIELDS} }} }}"
    );
    let data: BooksData =
        post_graphql(config, &query, serde_json::json!({ "title": title })).await?;
    let want_author = author.map(|a| a.trim().to_lowercase());
    let pick = data
        .books
        .iter()
        .find(|b| match &want_author {
            Some(wa) => b.contributions.iter().any(|c| {
                c.author
                    .as_ref()
                    .and_then(|a| a.name.as_deref())
                    .is_some_and(|n| n.trim().to_lowercase() == *wa)
            }),
            None => true,
        })
        .or_else(|| data.books.first());
    Ok(pick.map(resolved_from_row))
}

/// One ISBN lookup: resolve its edition to a `book_id`, then hydrate the full
/// `ResolvedBook`. `Ok(None)` means this ISBN alone had no match — the caller
/// tries the next one.
async fn resolve_by_isbn(
    config: &HardcoverConfig,
    isbn: &str,
) -> Result<Option<ResolvedBook>, HardcoverError> {
    let query = "query ($isbn: String!) { \
         editions(where: {_or: [{isbn_13: {_eq: $isbn}}, {isbn_10: {_eq: $isbn}}]}, limit: 1) \
         { book_id } }";
    let data: EditionsData =
        post_graphql(config, query, serde_json::json!({ "isbn": isbn })).await?;
    match data.editions.into_iter().find_map(|e| e.book_id) {
        Some(book_id) => fetch_resolved_book(config, book_id).await,
        None => Ok(None),
    }
}

fn resolved_from_row(b: &BookRow) -> ResolvedBook {
    ResolvedBook {
        id: b.id,
        author_names: author_names(b),
        series: series_name(b),
    }
}

fn author_names(b: &BookRow) -> Vec<String> {
    b.contributions
        .iter()
        .filter_map(|c| c.author.as_ref().and_then(|a| a.name.clone()))
        .collect()
}

fn series_name(b: &BookRow) -> Option<String> {
    b.book_series
        .first()
        .and_then(|bs| bs.series.as_ref().and_then(|s| s.name.clone()))
}

async fn fetch_resolved_book(
    config: &HardcoverConfig,
    book_id: i64,
) -> Result<Option<ResolvedBook>, HardcoverError> {
    let query = format!(
        "query {{ books(where: {{id: {{_eq: {book_id}}}}}, limit: 1) {{ {BOOK_FIELDS} }} }}"
    );
    let data: BooksData = post_graphql(config, &query, serde_json::json!({})).await?;
    Ok(data.books.first().map(resolved_from_row))
}

/// Rich single-book detail for the metadata-edit "Fetch from Hardcover"
/// action: title/authors/series (the same fields [`ResolvedBook`] carries)
/// plus the description and a representative edition's ISBN-13, which the
/// suggestions ranking path never needed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BookDetails {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub series: Option<String>,
    /// Book-number position formatted for display (e.g. `"1"`, `"2.5"`),
    /// or `None` when the book has no series or no known position.
    pub series_index: Option<String>,
    pub isbn13: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EditionIsbnData {
    editions: Vec<EditionIsbnRow>,
}
#[derive(Debug, Deserialize)]
struct EditionIsbnRow {
    #[serde(default)]
    isbn_13: Option<String>,
}

/// Fetch [`BookDetails`] for a resolved Hardcover book id — the same fields
/// query as [`resolve_book`]'s `books` lookup plus `description`, and a
/// second small query for a representative edition's ISBN-13 (Hardcover has
/// no single "primary edition" scalar to read it from directly). `Ok(None)`
/// when the id no longer resolves (deleted/merged on Hardcover's side
/// between resolution and this call).
pub async fn fetch_book_details(
    config: &HardcoverConfig,
    book_id: i64,
) -> Result<Option<BookDetails>, HardcoverError> {
    let query = format!(
        "query {{ books(where: {{id: {{_eq: {book_id}}}}}, limit: 1) {{ {BOOK_FIELDS} description }} }}"
    );
    let data: BooksData = post_graphql(config, &query, serde_json::json!({})).await?;
    let Some(b) = data.books.into_iter().next() else {
        return Ok(None);
    };
    let isbn13 = fetch_primary_isbn13(config, book_id).await?;
    let series_row = b.book_series.first();
    Ok(Some(BookDetails {
        title: b.title.clone(),
        authors: author_names(&b),
        description: b
            .description
            .clone()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        series: series_name(&b),
        series_index: series_row.and_then(|s| s.position).map(format_series_index),
        isbn13,
    }))
}

/// A representative edition's ISBN-13 for `book_id`, preferring the edition
/// with the most readers (mirrors the `users_count desc` ordering
/// [`resolve_book`]'s title fallback uses to prefer the canonical edition).
/// `Ok(None)` when no edition carries an ISBN-13.
async fn fetch_primary_isbn13(
    config: &HardcoverConfig,
    book_id: i64,
) -> Result<Option<String>, HardcoverError> {
    let query = format!(
        "query {{ editions(where: {{book_id: {{_eq: {book_id}}}, isbn_13: {{_is_null: false}}}}, \
         order_by: {{users_count: desc}}, limit: 1) {{ isbn_13 }} }}"
    );
    let data: EditionIsbnData = post_graphql(config, &query, serde_json::json!({})).await?;
    Ok(data.editions.into_iter().find_map(|e| e.isbn_13))
}

/// Format a Hardcover series position for display: whole numbers drop the
/// trailing `.0` (`1.0` → `"1"`); fractional positions (half-books, novella
/// insertions) keep their decimal (`2.5` → `"2.5"`).
fn format_series_index(position: f64) -> String {
    if position.fract() == 0.0 {
        format!("{}", position as i64)
    } else {
        position.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct BookDescData {
    books: Vec<BookDescRow>,
}
#[derive(Debug, Deserialize)]
struct BookDescRow {
    #[serde(default)]
    description: Option<String>,
}

/// Fetch the long-form `description` for a resolved Hardcover book id. A
/// blank/whitespace description is treated as absent (`Ok(None)`) so the caller
/// can fall back to another source. The tagline-style `headline` is
/// intentionally *not* used — it's a pull-quote, not a summary.
pub async fn book_description(
    config: &HardcoverConfig,
    book_id: i64,
) -> Result<Option<String>, HardcoverError> {
    let query =
        format!("query {{ books(where: {{id: {{_eq: {book_id}}}}}, limit: 1) {{ description }} }}");
    let data: BookDescData = post_graphql(config, &query, serde_json::json!({})).await?;
    Ok(data
        .books
        .into_iter()
        .find_map(|b| b.description)
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty()))
}

// ── List co-occurrence ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListBooksData {
    list_books: Vec<ListBookRow>,
}
#[derive(Debug, Deserialize)]
struct ListBookRow {
    #[serde(default)]
    list_id: Option<i64>,
    #[serde(default)]
    book_id: Option<i64>,
}

/// The curated, public lists (size band 5–150) a book sits on, largest first,
/// capped at [`LIST_SAMPLE`].
pub async fn curated_list_ids(
    config: &HardcoverConfig,
    book_id: i64,
) -> Result<Vec<i64>, HardcoverError> {
    let query = format!(
        "query {{ list_books(where: {{book_id: {{_eq: {book_id}}}, \
         list: {{books_count: {{_gte: {MIN_LIST_BOOKS}, _lte: {MAX_LIST_BOOKS}}}, \
         privacy_setting_id: {{_eq: {PRIVACY_PUBLIC}}}}}}}, \
         order_by: {{list: {{books_count: desc}}}}, limit: {LIST_SAMPLE}) {{ list_id }} }}"
    );
    let data: ListBooksData = post_graphql(config, &query, serde_json::json!({})).await?;
    Ok(data
        .list_books
        .into_iter()
        .filter_map(|r| r.list_id)
        .collect())
}

/// Rank every other book by how many of `list_ids` it co-appears on. Paginates
/// the members in [`PAGE_SIZE`] chunks up to [`MAX_MEMBER_ROWS`], excludes the
/// source book, and returns `(book_id, count)` sorted by count desc, truncated
/// to [`CANDIDATE_POOL`].
pub async fn co_listed_counts(
    config: &HardcoverConfig,
    list_ids: &[i64],
    exclude_book_id: i64,
) -> Result<Vec<(i64, i64)>, HardcoverError> {
    if list_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids_csv = list_ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut counts: HashMap<i64, i64> = HashMap::new();
    let mut offset: i64 = 0;
    let mut pulled: usize = 0;
    loop {
        let query = format!(
            "query {{ list_books(where: {{list_id: {{_in: [{ids_csv}]}}}}, \
             order_by: {{id: asc}}, limit: {PAGE_SIZE}, offset: {offset}) {{ book_id }} }}"
        );
        let data: ListBooksData = post_graphql(config, &query, serde_json::json!({})).await?;
        let n = data.list_books.len();
        for row in data.list_books {
            if let Some(bid) = row.book_id {
                if bid != exclude_book_id {
                    *counts.entry(bid).or_insert(0) += 1;
                }
            }
        }
        pulled += n;
        if (n as i64) < PAGE_SIZE || pulled >= MAX_MEMBER_ROWS {
            break;
        }
        offset += PAGE_SIZE;
    }
    let mut ranked: Vec<(i64, i64)> = counts.into_iter().collect();
    // Sort by count desc, then book_id asc for a deterministic tie-break.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(CANDIDATE_POOL);
    Ok(ranked)
}

/// Hydrate the top co-listed `book_id`s into [`Candidate`]s, attaching each
/// book's `list_count` from the ranking. Order follows `ranked` (count desc).
pub async fn fetch_candidates(
    config: &HardcoverConfig,
    ranked: &[(i64, i64)],
) -> Result<Vec<Candidate>, HardcoverError> {
    if ranked.is_empty() {
        return Ok(Vec::new());
    }
    let ids_csv = ranked
        .iter()
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let query =
        format!("query {{ books(where: {{id: {{_in: [{ids_csv}]}}}}) {{ {BOOK_FIELDS} }} }}");
    let data: BooksData = post_graphql(config, &query, serde_json::json!({})).await?;

    let count_by_id: HashMap<i64, i64> = ranked.iter().copied().collect();
    let by_id: HashMap<i64, BookRow> = data.books.into_iter().map(|b| (b.id, b)).collect();

    // Walk `ranked` so output preserves the co-listing order.
    let mut out = Vec::new();
    for (id, _) in ranked {
        if let Some(b) = by_id.get(id) {
            let series = b.book_series.first();
            out.push(Candidate {
                hardcover_id: b.id,
                slug: b.slug.clone(),
                title: b.title.clone().unwrap_or_default(),
                author: author_names(b).into_iter().next().unwrap_or_default(),
                series: series.and_then(|s| s.series.as_ref().and_then(|s| s.name.clone())),
                series_position: series.and_then(|s| s.position),
                list_count: count_by_id.get(id).copied().unwrap_or(0),
                cover_url: b.image.as_ref().and_then(|i| i.url.clone()),
            });
        }
    }
    Ok(out)
}
