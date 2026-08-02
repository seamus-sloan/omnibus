//! HTTP clients for the two ISBN metadata providers. Each maps a provider's
//! JSON into the shared [`ExternalBookMeta`]. A clean miss (the provider
//! answered but held no match) is `Ok(None)`; transport/parse failures are
//! `anyhow::Error` with context — the open-ended, foreign-system failure space.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use serde::Deserialize;

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
        source: MetadataProvider::OpenLibrary,
    }))
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
    let Some(title) = info.title.filter(|t| !t.trim().is_empty()) else {
        return Ok(None);
    };
    // Google Books serves image links over `http://`; upgrade to `https://`
    // so the cover isn't blocked as mixed content (or by the https-only CSP
    // img-src host allowlist) when previewed on the scan result page.
    let cover_url = info
        .image_links
        .and_then(|l| l.thumbnail.or(l.small_thumbnail))
        .map(|u| upgrade_to_https(&u));

    Ok(Some(ExternalBookMeta {
        isbn13: isbn13.to_string(),
        title,
        authors: info.authors,
        year: info.published_date.as_deref().and_then(publication_year),
        // Google returns 0 for "unknown", which would render as a book with
        // zero pages rather than one whose length we don't know.
        pages: info.page_count.filter(|p| *p > 0),
        publisher: info.publisher,
        description: info.description,
        cover_url,
        source: MetadataProvider::GoogleBooks,
    }))
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
