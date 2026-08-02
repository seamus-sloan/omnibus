//! HTTP clients for the two ISBN metadata providers. Each maps a provider's
//! JSON into the shared [`ExternalBookMeta`]. A clean miss (the provider
//! answered but held no match) is `Ok(None)`; transport/parse failures are
//! `anyhow::Error` with context — the open-ended, foreign-system failure space.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use serde::Deserialize;

use super::SEARCH_LIMIT;

/// Per-request timeout for a provider call. A single lookup runs per scan, so
/// this bounds how long the check-in flow waits on a slow provider.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

/// Environment variable holding the Google Books API key.
const GOOGLE_BOOKS_API_KEY_ENV: &str = "GOOGLE_BOOKS_API_KEY";

/// Connection config for the metadata providers. Base URLs are injectable so
/// tests point them at a local `wiremock` server.
#[derive(Debug, Clone)]
pub struct MetadataLookupConfig {
    /// `https://openlibrary.org` in production.
    pub openlibrary_base: String,
    /// `https://www.googleapis.com` in production.
    pub googlebooks_base: String,
    /// Google Books API key. Optional: without one the API still answers, but
    /// on a shared anonymous daily quota that a self-hosted instance will hit
    /// (HTTP 429) — in practice keyless lookups fail more often than they
    /// succeed. Never logged; see [`strip_url`].
    pub googlebooks_api_key: Option<String>,
    /// Per-request timeout applied to each provider HTTP call.
    pub timeout: Duration,
}

impl MetadataLookupConfig {
    /// Config pointing at the live provider endpoints, with the Google Books
    /// key taken from the environment when set.
    ///
    /// Prefer [`live_with_key`](Self::live_with_key) from a request path that
    /// has a DB pool: the saved Settings key takes precedence over the env var
    /// (see `effective_google_books_api_key`). This env-only form is the
    /// fallback for contexts without a pool (`Default`, tests).
    pub fn live() -> Self {
        Self::live_with_key(
            std::env::var(GOOGLE_BOOKS_API_KEY_ENV)
                .ok()
                .filter(|k| !k.trim().is_empty()),
        )
    }

    /// Config pointing at the live provider endpoints with an already-resolved
    /// Google Books key. Callers pass the effective key from settings so the
    /// saved value wins over the env var without this crate needing a pool.
    pub fn live_with_key(googlebooks_api_key: Option<String>) -> Self {
        Self {
            openlibrary_base: "https://openlibrary.org".to_string(),
            googlebooks_base: "https://www.googleapis.com".to_string(),
            googlebooks_api_key,
            timeout: LOOKUP_TIMEOUT,
        }
    }
}

impl Default for MetadataLookupConfig {
    fn default() -> Self {
        Self::live()
    }
}

/// Process-wide `reqwest::Client`, built once and cloned so lookups share one
/// connection pool + TLS session cache. Fallible (TLS backend init).
fn client() -> reqwest::Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let new = crate::http_client::build_client(&crate::http_client::default_user_agent())?;
    Ok(CLIENT.get_or_init(|| new).clone())
}

/// Drop the URL from a `reqwest::Error` before it reaches a log.
///
/// Google's API takes its key as a `?key=` query parameter, and a
/// `reqwest::Error` renders the full request URL in its `Display` — so a plain
/// `?` on a 429 would write the key into `omnibus.log`. The status and kind
/// are what diagnose a provider failure; the URL is not.
fn strip_url(e: reqwest::Error) -> reqwest::Error {
    e.without_url()
}

/// Reduce a publication date to its year.
///
/// Google Books returns `publishedDate` in whatever precision it holds —
/// `"2025"`, `"2025-02"`, or `"2025-02-25"` — while Open Library gives a bare
/// year. The chooser card renders this next to the title ("Dune · 2005"), so
/// an un-trimmed value makes two providers look like two different fields.
/// Anything that doesn't start with four digits is passed through untouched
/// rather than guessed at.
pub(super) fn publication_year(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let head: String = trimmed.chars().take(4).collect();
    if head.len() == 4 && head.chars().all(|c| c.is_ascii_digit()) {
        return Some(head);
    }
    Some(trimmed.to_string())
}

/// Backoff between Google Books retries. Length + 1 is the attempt count.
/// Deliberately short: a check-in scan is waiting on a spinner, and the
/// failures this covers come back fast.
const GB_RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(600)];

/// Whether a Google Books status is worth another attempt. Observed in the
/// wild: the API intermittently answers `503 backendFailed` for perfectly
/// valid ISBNs (roughly half of calls during an episode), and 429 is the
/// quota rung. Both are Google telling us to come back, not an answer.
fn gb_is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// GET from Google Books, retrying a retryable status a couple of times.
///
/// Only a *received status* is retried — a timeout or transport error is not,
/// since those already cost the full per-request budget and retrying them
/// would leave the scan flow spinning for half a minute.
async fn googlebooks_get(
    config: &MetadataLookupConfig,
    url: &str,
) -> anyhow::Result<reqwest::Response> {
    let mut last: Option<reqwest::StatusCode> = None;
    for attempt in 0..=GB_RETRY_BACKOFF.len() {
        let resp = client()?
            .get(url)
            .timeout(config.timeout)
            .send()
            .await
            .map_err(strip_url)
            .context("google books request failed")?;

        if !gb_is_retryable(resp.status()) {
            return resp
                .error_for_status()
                .map_err(strip_url)
                .context("google books returned an error status");
        }
        last = Some(resp.status());
        if let Some(backoff) = GB_RETRY_BACKOFF.get(attempt) {
            tokio::time::sleep(*backoff).await;
        }
    }
    // Status only — never the URL, which carries the API key.
    anyhow::bail!(
        "google books unavailable after {} attempts (last status {})",
        GB_RETRY_BACKOFF.len() + 1,
        last.map_or(0, |s| s.as_u16())
    )
}

/// Build the Google Books volumes URL, appending the API key when configured.
///
/// Kept separate so a test can assert the key is attached (and absent when
/// unset) without a live request.
pub(super) fn googlebooks_url(config: &MetadataLookupConfig, isbn13: &str) -> String {
    googlebooks_volumes_url(config, &format!("isbn:{isbn13}"))
}

/// Build the bare-text variant of the volumes URL (`q=<isbn13>`, no `isbn:`
/// field restriction) for the fallback in [`googlebooks_lookup`].
pub(super) fn googlebooks_bare_url(config: &MetadataLookupConfig, isbn13: &str) -> String {
    googlebooks_volumes_url(config, isbn13)
}

fn googlebooks_volumes_url(config: &MetadataLookupConfig, q: &str) -> String {
    let base = format!("{}/books/v1/volumes?q={q}", config.googlebooks_base);
    match config.googlebooks_api_key.as_deref() {
        Some(key) => format!("{base}&key={key}"),
        None => base,
    }
}

// ── Open Library ─────────────────────────────────────────────────

/// The `jscmd=data` response is a map keyed `"ISBN:<isbn>"`; a missing key means
/// the ISBN isn't known.
#[derive(Debug, Deserialize)]
struct OlBook {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authors: Vec<OlNamed>,
    #[serde(default)]
    publish_date: Option<String>,
    #[serde(default)]
    number_of_pages: Option<i64>,
    #[serde(default)]
    publishers: Vec<OlNamed>,
    #[serde(default)]
    cover: Option<OlCover>,
}

#[derive(Debug, Deserialize)]
struct OlNamed {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OlCover {
    #[serde(default)]
    large: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    small: Option<String>,
}

/// Look up an ISBN-13 against Open Library's `api/books?jscmd=data` endpoint.
/// `Ok(None)` when the ISBN is unknown; `Err` on transport/parse failure.
pub async fn openlibrary_lookup(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    let key = format!("ISBN:{isbn13}");
    // isbn13 is digit-only (post-normalization), so no query-value encoding is
    // needed; the `:` in the bibkey is a legal query char.
    let url = format!(
        "{}/api/books?bibkeys={key}&format=json&jscmd=data",
        config.openlibrary_base
    );
    let resp = client()?
        .get(&url)
        .timeout(config.timeout)
        .send()
        .await
        .context("open library request failed")?
        .error_for_status()
        .context("open library returned an error status")?;

    let mut body: std::collections::HashMap<String, OlBook> = resp
        .json()
        .await
        .context("open library response was not valid json")?;

    let Some(book) = body.remove(&key) else {
        return Ok(None);
    };
    let Some(title) = book.title.filter(|t| !t.trim().is_empty()) else {
        // A record with no title is unusable — treat as a miss.
        return Ok(None);
    };
    let cover_url = book.cover.and_then(|c| c.large.or(c.medium).or(c.small));

    Ok(Some(ExternalBookMeta {
        isbn13: isbn13.to_string(),
        title,
        authors: book.authors.into_iter().filter_map(|a| a.name).collect(),
        year: book.publish_date,
        pages: book.number_of_pages,
        publisher: book.publishers.into_iter().find_map(|p| p.name),
        description: None,
        cover_url,
        series: None,
        first_publish_year: None,
        source: MetadataProvider::OpenLibrary,
    }))
}

// ── Open Library title search ────────────────────────────────────

/// `search.json` answers works, not editions: `isbn` carries every edition's
/// identifiers and `first_publish_year` is computed across all of them.
#[derive(Debug, Deserialize)]
struct OlSearchResponse {
    #[serde(default)]
    docs: Vec<OlSearchDoc>,
}

#[derive(Debug, Deserialize)]
struct OlSearchDoc {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author_name: Vec<String>,
    #[serde(default)]
    first_publish_year: Option<i64>,
    #[serde(default)]
    isbn: Vec<String>,
    #[serde(default)]
    cover_i: Option<i64>,
    #[serde(default)]
    number_of_pages_median: Option<i64>,
}

/// Build the Open Library title-search URL. The query is user text, so it goes
/// through `Url`'s percent-encoding rather than string interpolation.
pub(super) fn openlibrary_search_url(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(&format!("{}/search.json", config.openlibrary_base))
        .context("invalid open library base url")?;
    url.query_pairs_mut()
        .append_pair("title", query)
        .append_pair(
            "fields",
            "title,author_name,first_publish_year,isbn,cover_i,number_of_pages_median",
        )
        .append_pair("limit", &SEARCH_LIMIT.to_string());
    Ok(url.into())
}

/// Search Open Library by title text. `Ok(empty)` on no matches; `Err` on
/// transport/parse failure.
pub async fn openlibrary_search(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ExternalBookMeta>> {
    let url = openlibrary_search_url(config, query)?;
    let resp = client()?
        .get(&url)
        .timeout(config.timeout)
        .send()
        .await
        .context("open library request failed")?
        .error_for_status()
        .context("open library returned an error status")?;
    let body: OlSearchResponse = resp
        .json()
        .await
        .context("open library response was not valid json")?;
    Ok(body
        .docs
        .into_iter()
        .filter_map(map_ol_search_doc)
        .collect())
}

/// Map one search doc into `ExternalBookMeta`. A doc without a title or a
/// valid ISBN maps to `None` — the whole check-in flow keys on the ISBN
/// downstream (stored per copy, printed on the confirm screens), so a
/// candidate that can't supply one isn't actionable.
fn map_ol_search_doc(doc: OlSearchDoc) -> Option<ExternalBookMeta> {
    let title = doc.title.filter(|t| !t.trim().is_empty())?;
    let isbn13 = doc.isbn.iter().find_map(|v| normalize_isbn(v).ok())?;
    Some(ExternalBookMeta {
        isbn13,
        title,
        authors: doc.author_name,
        year: None,
        pages: doc.number_of_pages_median.filter(|p| *p > 0),
        publisher: None,
        description: None,
        // Work-level cover id; the covers host is fixed in production and the
        // URL is only fetched at create time (SSRF-guarded), never at search.
        cover_url: doc
            .cover_i
            .map(|id| format!("https://covers.openlibrary.org/b/id/{id}-L.jpg")),
        series: None,
        first_publish_year: doc.first_publish_year,
        source: MetadataProvider::OpenLibrary,
    })
}

// ── Open Library enrichment ──────────────────────────────────────

/// Bonus fields for an already-resolved ISBN, filled best-effort from Open
/// Library: the edition's series statement and the work's first-publish year.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OlEnrichment {
    pub series: Option<String>,
    pub first_publish_year: Option<i64>,
}

/// Fetch [`OlEnrichment`] for an ISBN. Never fails — a provider hiccup or an
/// ISBN Open Library doesn't know just means fewer fields. The two lookups
/// run concurrently and each is bounded by the config timeout.
pub async fn openlibrary_enrich(config: &MetadataLookupConfig, isbn13: &str) -> OlEnrichment {
    let (series, first_publish_year) = tokio::join!(
        edition_series(config, isbn13),
        work_first_publish_year(config, isbn13)
    );
    OlEnrichment {
        series,
        first_publish_year,
    }
}

/// The edition record (`/isbn/<isbn>.json`) is the only Open Library surface
/// that carries a series statement.
async fn edition_series(config: &MetadataLookupConfig, isbn13: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct OlEdition {
        #[serde(default)]
        series: Vec<String>,
    }
    let url = format!("{}/isbn/{isbn13}.json", config.openlibrary_base);
    let edition: OlEdition = get_json_best_effort(config, &url).await?;
    // An oversized statement is dropped rather than truncated: the meta is
    // posted back on the write paths, where `validate` would reject it.
    edition
        .series
        .into_iter()
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty() && s.chars().count() <= ExternalBookMeta::NAME_MAX_LEN)
}

/// `first_publish_year` lives on the work, surfaced through the search API's
/// `isbn:` field query.
async fn work_first_publish_year(config: &MetadataLookupConfig, isbn13: &str) -> Option<i64> {
    #[derive(Deserialize)]
    struct Docs {
        #[serde(default)]
        docs: Vec<Doc>,
    }
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        first_publish_year: Option<i64>,
    }
    // isbn13 is digit-only (post-normalization), so no query encoding needed.
    let url = format!(
        "{}/search.json?q=isbn:{isbn13}&fields=first_publish_year&limit=1",
        config.openlibrary_base
    );
    let body: Docs = get_json_best_effort(config, &url).await?;
    body.docs
        .into_iter()
        .next()
        .and_then(|d| d.first_publish_year)
}

/// GET + parse JSON, degrading every failure to `None` with a debug log —
/// enrichment is strictly best-effort and must never fail a scan.
async fn get_json_best_effort<T: serde::de::DeserializeOwned>(
    config: &MetadataLookupConfig,
    url: &str,
) -> Option<T> {
    let result: anyhow::Result<T> = async {
        let resp = client()?
            .get(url)
            .timeout(config.timeout)
            .send()
            .await
            .map_err(strip_url)?
            .error_for_status()
            .map_err(strip_url)?;
        Ok(resp.json::<T>().await.map_err(strip_url)?)
    }
    .await;
    match result {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("open library enrichment miss: {e:#}");
            None
        }
    }
}

// ── Google Books ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GbResponse {
    #[serde(default)]
    items: Vec<GbItem>,
}

#[derive(Debug, Deserialize)]
struct GbItem {
    #[serde(rename = "volumeInfo", default)]
    volume_info: Option<GbVolumeInfo>,
}

#[derive(Debug, Deserialize, Default)]
struct GbVolumeInfo {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(rename = "publishedDate", default)]
    published_date: Option<String>,
    #[serde(rename = "pageCount", default)]
    page_count: Option<i64>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "imageLinks", default)]
    image_links: Option<GbImageLinks>,
    #[serde(rename = "industryIdentifiers", default)]
    industry_identifiers: Vec<GbIndustryId>,
}

#[derive(Debug, Deserialize)]
struct GbIndustryId {
    #[serde(default)]
    identifier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GbImageLinks {
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(rename = "smallThumbnail", default)]
    small_thumbnail: Option<String>,
}

/// Look up an ISBN-13 against Google Books `volumes?q=isbn:`, falling back to
/// a bare-text `q=<isbn13>` query when the field search answers empty and a
/// key is configured. `Ok(None)` when no volume matches either way (or the
/// fallback is skipped keyless); `Err` on transport/parse failure of the
/// field query (the bare query is best-effort — see below).
pub async fn googlebooks_lookup(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    if let Some(meta) = googlebooks_query(config, isbn13, &googlebooks_url(config, isbn13)).await? {
        return Ok(Some(meta));
    }
    // Keyless requests share Google's anonymous daily quota (see
    // .claude/rules/01-dev-environment.md) and exhaust it almost
    // immediately, so doubling every miss into a second request is a cost
    // only a keyed instance can afford — gate the fallback on one being
    // configured.
    if config.googlebooks_api_key.is_none() {
        return Ok(None);
    }
    // The `isbn:` field search has been observed answering 200/totalItems=0
    // for volumes the corpus demonstrably holds (2026-08: every field-scoped
    // query returned empty while the same bare-text query hit). The bare query
    // may surface a sibling edition, which is acceptable: the scan flow always
    // confirms with the user, and the stored ISBN is the scanned one either
    // way (`meta.isbn13` below). A bare-query *failure* degrades to the field
    // query's clean miss rather than erroring — without this fallback the
    // outcome would have been a miss anyway.
    match googlebooks_query(config, isbn13, &googlebooks_bare_url(config, isbn13)).await {
        Ok(meta) => {
            if meta.is_some() {
                tracing::info!(
                    isbn13,
                    "google books isbn: search missed but bare-text query hit — field search degraded upstream"
                );
            }
            Ok(meta)
        }
        Err(e) => {
            tracing::warn!(isbn13, "google books bare-text fallback failed: {e:#}");
            Ok(None)
        }
    }
}

/// Run one volumes query and map its first usable volume. `Ok(None)` when no
/// volume matches; `Err` on transport/parse failure.
async fn googlebooks_query(
    config: &MetadataLookupConfig,
    isbn13: &str,
    url: &str,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    let resp = googlebooks_get(config, url).await?;

    let body: GbResponse = resp
        .json()
        .await
        .context("google books response was not valid json")?;

    let Some(info) = body.items.into_iter().find_map(|i| i.volume_info) else {
        return Ok(None);
    };
    Ok(map_gb_volume(info, Some(isbn13)))
}

/// Map one Google Books volume into `ExternalBookMeta`. `isbn13` is the
/// caller's authoritative ISBN when it has one (the ISBN-lookup path stores
/// the *scanned* barcode); title search derives it from the volume's own
/// industry identifiers instead. A volume without a title, or a search result
/// without a valid ISBN, maps to `None` — the check-in flow keys on the ISBN
/// downstream.
fn map_gb_volume(info: GbVolumeInfo, isbn13: Option<&str>) -> Option<ExternalBookMeta> {
    let title = info.title.filter(|t| !t.trim().is_empty())?;
    let isbn13 = match isbn13 {
        Some(scanned) => scanned.to_string(),
        None => info
            .industry_identifiers
            .iter()
            .filter_map(|i| i.identifier.as_deref())
            .find_map(|v| normalize_isbn(v).ok())?,
    };
    // Google Books serves image links over `http://`; upgrade to `https://`
    // so the cover isn't blocked as mixed content (or by the https-only CSP
    // img-src host allowlist) when previewed on the scan result page.
    let cover_url = info
        .image_links
        .and_then(|l| l.thumbnail.or(l.small_thumbnail))
        .map(|u| upgrade_to_https(&u));

    Some(ExternalBookMeta {
        isbn13,
        title,
        authors: info.authors,
        year: info.published_date.as_deref().and_then(publication_year),
        // Google returns 0 for "unknown", which would render as a book with
        // zero pages rather than one whose length we don't know.
        pages: info.page_count.filter(|p| *p > 0),
        publisher: info.publisher,
        description: info.description,
        cover_url,
        series: None,
        first_publish_year: None,
        source: MetadataProvider::GoogleBooks,
    })
}

// ── Google Books title search ────────────────────────────────────

/// Build the Google Books title-search URL: a bare-text `q` (their general
/// search ranks title matches well and tolerates typos better than an
/// `intitle:` field query), books only, key appended when configured. The
/// query is user text, so it goes through `Url`'s percent-encoding.
pub(super) fn googlebooks_search_url(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(&format!("{}/books/v1/volumes", config.googlebooks_base))
        .context("invalid google books base url")?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("printType", "books")
        .append_pair("maxResults", &SEARCH_LIMIT.to_string());
    if let Some(key) = config.googlebooks_api_key.as_deref() {
        url.query_pairs_mut().append_pair("key", key);
    }
    Ok(url.into())
}

/// Search Google Books by title text. `Ok(empty)` on no usable volumes; `Err`
/// on transport/parse failure.
pub async fn googlebooks_search(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ExternalBookMeta>> {
    let url = googlebooks_search_url(config, query)?;
    let resp = googlebooks_get(config, &url).await?;
    let body: GbResponse = resp
        .json()
        .await
        .context("google books response was not valid json")?;
    Ok(body
        .items
        .into_iter()
        .filter_map(|i| i.volume_info)
        .filter_map(|info| map_gb_volume(info, None))
        .collect())
}

/// Upgrade an `http://` URL to `https://`; other schemes pass through
/// unchanged. Google Books returns cover links over plain HTTP, which a
/// browser blocks as mixed content on an HTTPS page.
fn upgrade_to_https(url: &str) -> String {
    match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::upgrade_to_https;

    #[test]
    fn upgrade_to_https_rewrites_http_and_leaves_others() {
        assert_eq!(
            upgrade_to_https("http://books.google.com/x.jpg"),
            "https://books.google.com/x.jpg"
        );
        // Already-secure and non-http schemes pass through untouched.
        assert_eq!(
            upgrade_to_https("https://covers.openlibrary.org/b/id/1-L.jpg"),
            "https://covers.openlibrary.org/b/id/1-L.jpg"
        );
        assert_eq!(
            upgrade_to_https("data:image/png;base64,AAAA"),
            "data:image/png;base64,AAAA"
        );
    }
}
