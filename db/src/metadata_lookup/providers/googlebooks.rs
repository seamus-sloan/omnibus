//! Google Books provider: the `by_isbn` / `by_title` pair every provider in
//! this directory implements.
//!
//! Reachable without a key, but only just — keyless requests share Google's
//! anonymous daily quota, which a self-hosted instance exhausts almost
//! immediately (HTTP 429). A configured key is what makes it the ladder's
//! primary rung.

use std::time::Duration;

use anyhow::Context;
use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use serde::Deserialize;

use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use super::http::{base_url, client, publication_year, strip_url, upgrade_to_https};

/// Backoff between retries. Length + 1 is the attempt count. Deliberately
/// short: a check-in scan is waiting on a spinner, and the failures this
/// covers come back fast.
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(600)];

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

/// Whether a status is worth another attempt. Observed in the wild: the API
/// intermittently answers `503 backendFailed` for perfectly valid ISBNs
/// (roughly half of calls during an episode), and 429 is the quota rung. Both
/// are Google telling us to come back, not an answer.
fn is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// GET, retrying a retryable status a couple of times.
///
/// Only a *received status* is retried — a timeout or transport error is not,
/// since those already cost the full per-request budget and retrying them
/// would leave the scan flow spinning for half a minute.
async fn get(config: &MetadataLookupConfig, url: &str) -> anyhow::Result<reqwest::Response> {
    let mut last: Option<reqwest::StatusCode> = None;
    for attempt in 0..=RETRY_BACKOFF.len() {
        let resp = client()?
            .get(url)
            .timeout(config.timeout)
            .send()
            .await
            .map_err(strip_url)
            .context("google books request failed")?;

        if !is_retryable(resp.status()) {
            return resp
                .error_for_status()
                .map_err(strip_url)
                .context("google books returned an error status");
        }
        last = Some(resp.status());
        if let Some(backoff) = RETRY_BACKOFF.get(attempt) {
            tokio::time::sleep(*backoff).await;
        }
    }
    // Status only — never the URL, which carries the API key.
    anyhow::bail!(
        "google books unavailable after {} attempts (last status {})",
        RETRY_BACKOFF.len() + 1,
        last.map_or(0, |s| s.as_u16())
    )
}

/// Build the volumes URL for an `isbn:`-scoped query.
///
/// Kept separate so a test can assert the key is attached (and absent when
/// unset) without a live request.
pub(in crate::metadata_lookup) fn isbn_url(config: &MetadataLookupConfig, isbn13: &str) -> String {
    volumes_url(config, &format!("isbn:{isbn13}"))
}

/// Build the bare-text variant of the volumes URL (`q=<isbn13>`, no `isbn:`
/// field restriction) for the fallback in [`by_isbn`].
pub(in crate::metadata_lookup) fn bare_url(config: &MetadataLookupConfig, isbn13: &str) -> String {
    volumes_url(config, isbn13)
}

/// `q` here is always server-built (`isbn:<digits>` or bare digits), never
/// user text — [`search_url`] is the percent-encoding path for that.
fn volumes_url(config: &MetadataLookupConfig, q: &str) -> String {
    let base = format!("{}/books/v1/volumes?q={q}", config.googlebooks_base);
    match config.keys.googlebooks.as_deref() {
        Some(key) => format!("{base}&key={key}"),
        None => base,
    }
}

/// Look up an ISBN-13 against `volumes?q=isbn:`, falling back to a bare-text
/// `q=<isbn13>` query when the field search answers empty and a key is
/// configured. `Ok(None)` when no volume matches either way (or the fallback
/// is skipped keyless); `Err` on transport/parse failure of the field query
/// (the bare query is best-effort — see below).
pub async fn by_isbn(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    if let Some(meta) = query_one(config, isbn13, &isbn_url(config, isbn13)).await? {
        return Ok(Some(meta));
    }
    // Keyless requests share Google's anonymous daily quota (see
    // .claude/rules/01-dev-environment.md) and exhaust it almost
    // immediately, so doubling every miss into a second request is a cost
    // only a keyed instance can afford — gate the fallback on one being
    // configured.
    if config.keys.googlebooks.is_none() {
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
    match query_one(config, isbn13, &bare_url(config, isbn13)).await {
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
async fn query_one(
    config: &MetadataLookupConfig,
    isbn13: &str,
    url: &str,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    let resp = get(config, url).await?;

    let body: GbResponse = resp
        .json()
        .await
        .context("google books response was not valid json")?;

    let Some(info) = body.items.into_iter().find_map(|i| i.volume_info) else {
        return Ok(None);
    };
    Ok(map_volume(info, Some(isbn13)))
}

/// Map one volume into `ExternalBookMeta`. `isbn13` is the caller's
/// authoritative ISBN when it has one (the ISBN-lookup path stores the
/// *scanned* barcode); title search derives it from the volume's own industry
/// identifiers instead. A volume without a title, or a search result without a
/// valid ISBN, maps to `None` — the check-in flow keys on the ISBN downstream.
fn map_volume(info: GbVolumeInfo, isbn13: Option<&str>) -> Option<ExternalBookMeta> {
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

// ── Title search ─────────────────────────────────────────────────

/// Build the title-search URL: a bare-text `q` (their general search ranks
/// title matches well and tolerates typos better than an `intitle:` field
/// query), books only, key appended when configured. The query is user text,
/// so it goes through `Url`'s percent-encoding.
pub(in crate::metadata_lookup) fn search_url(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<String> {
    let mut url = base_url(
        &config.googlebooks_base,
        "/books/v1/volumes",
        "google books",
    )?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("printType", "books")
        .append_pair("maxResults", &SEARCH_LIMIT.to_string());
    if let Some(key) = config.keys.googlebooks.as_deref() {
        url.query_pairs_mut().append_pair("key", key);
    }
    Ok(url.into())
}

/// Search by title text. `Ok(empty)` on no usable volumes; `Err` on
/// transport/parse failure.
pub async fn by_title(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ExternalBookMeta>> {
    let url = search_url(config, query)?;
    let resp = get(config, &url).await?;
    let body: GbResponse = resp
        .json()
        .await
        .context("google books response was not valid json")?;
    Ok(body
        .items
        .into_iter()
        .filter_map(|i| i.volume_info)
        .filter_map(|info| map_volume(info, None))
        .collect())
}
