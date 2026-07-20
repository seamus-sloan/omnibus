//! HTTP clients for the two ISBN metadata providers. Each maps a provider's
//! JSON into the shared [`ExternalBookMeta`]. A clean miss (the provider
//! answered but held no match) is `Ok(None)`; transport/parse failures are
//! `anyhow::Error` with context — the open-ended, foreign-system failure space.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};

/// Per-request timeout for a provider call. A single lookup runs per scan, so
/// this bounds how long the check-in flow waits on a slow provider.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

/// Connection config for the metadata providers. Base URLs are injectable so
/// tests point them at a local `wiremock` server.
#[derive(Debug, Clone)]
pub struct MetadataLookupConfig {
    /// `https://openlibrary.org` in production.
    pub openlibrary_base: String,
    /// `https://www.googleapis.com` in production.
    pub googlebooks_base: String,
    pub timeout: Duration,
}

impl MetadataLookupConfig {
    /// Config pointing at the live provider endpoints.
    pub fn live() -> Self {
        Self {
            openlibrary_base: "https://openlibrary.org".to_string(),
            googlebooks_base: "https://www.googleapis.com".to_string(),
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
    let new = reqwest::Client::builder()
        .user_agent(format!(
            "omnibus/{} (https://github.com/sloansa/omnibus)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;
    let _ = CLIENT.set(new.clone());
    Ok(CLIENT.get().cloned().unwrap_or(new))
}

/// Fetch a provider-hosted cover image, returning `(content_type, bytes)`.
/// `None` on any failure — a missing cover must never fail a check-in, and the
/// covers pipeline treats a book with no file as simply "no cover".
pub async fn fetch_cover(url: &str) -> Option<(String, Vec<u8>)> {
    let resp = client()
        .ok()?
        .get(url)
        .timeout(LOOKUP_TIMEOUT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = resp.bytes().await.ok()?.to_vec();
    (!bytes.is_empty()).then_some((mime, bytes))
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

/// Look up an ISBN-13 against Google Books `volumes?q=isbn:`. `Ok(None)` when no
/// volume matches; `Err` on transport/parse failure.
pub async fn googlebooks_lookup(
    config: &MetadataLookupConfig,
    isbn13: &str,
) -> anyhow::Result<Option<ExternalBookMeta>> {
    let url = format!(
        "{}/books/v1/volumes?q=isbn:{isbn13}",
        config.googlebooks_base
    );
    let resp = client()?
        .get(&url)
        .timeout(config.timeout)
        .send()
        .await
        .context("google books request failed")?
        .error_for_status()
        .context("google books returned an error status")?;

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
    let cover_url = info
        .image_links
        .and_then(|l| l.thumbnail.or(l.small_thumbnail));

    Ok(Some(ExternalBookMeta {
        isbn13: isbn13.to_string(),
        title,
        authors: info.authors,
        year: info.published_date,
        pages: info.page_count,
        publisher: info.publisher,
        description: info.description,
        cover_url,
        source: MetadataProvider::GoogleBooks,
    }))
}
