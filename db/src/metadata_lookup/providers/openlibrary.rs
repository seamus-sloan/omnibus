//! Open Library provider: the `by_isbn` / `by_title` pair every provider in
//! this directory implements, plus the `enrich` side lookup only this one can
//! serve (no other provider exposes an edition's series statement).
//!
//! Needs no API key, so it is always a reachable rung.

use anyhow::Context;
use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use serde::Deserialize;

use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use super::http::{base_url, client, get_json_best_effort};

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
pub async fn by_isbn(
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

// ── Title search ─────────────────────────────────────────────────

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

/// Build the title-search URL. The query is user text, so it goes through
/// `Url`'s percent-encoding rather than string interpolation.
pub(in crate::metadata_lookup) fn search_url(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<String> {
    let mut url = base_url(&config.openlibrary_base, "/search.json", "open library")?;
    url.query_pairs_mut()
        .append_pair("title", query)
        .append_pair(
            "fields",
            "title,author_name,first_publish_year,isbn,cover_i,number_of_pages_median",
        )
        .append_pair("limit", &SEARCH_LIMIT.to_string());
    Ok(url.into())
}

/// Search by title text. `Ok(empty)` on no matches; `Err` on transport/parse
/// failure.
pub async fn by_title(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ExternalBookMeta>> {
    let url = search_url(config, query)?;
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
    Ok(body.docs.into_iter().filter_map(map_search_doc).collect())
}

/// Map one search doc into `ExternalBookMeta`. A doc without a title or a
/// valid ISBN maps to `None` — the whole check-in flow keys on the ISBN
/// downstream (stored per copy, printed on the confirm screens), so a
/// candidate that can't supply one isn't actionable.
fn map_search_doc(doc: OlSearchDoc) -> Option<ExternalBookMeta> {
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

// ── Enrichment ───────────────────────────────────────────────────

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
///
/// `isbn13` is interpolated into a URL path, so callers must pass a
/// **canonicalized** ISBN; the percent-encoding here is the second line of
/// defence, not a licence to pass wire input straight through.
pub async fn enrich(config: &MetadataLookupConfig, isbn13: &str) -> OlEnrichment {
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
    // `path_segments_mut` percent-encodes the segment, so a value that somehow
    // reached here unvalidated addresses a 404 rather than an endpoint of its
    // own choosing.
    let mut url = base_url(&config.openlibrary_base, "/isbn", "open library").ok()?;
    url.path_segments_mut()
        .ok()?
        .push(&format!("{isbn13}.json"));
    let edition: OlEdition = get_json_best_effort(config, url.as_str()).await?;
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
    let mut url = base_url(&config.openlibrary_base, "/search.json", "open library").ok()?;
    url.query_pairs_mut()
        .append_pair("q", &format!("isbn:{isbn13}"))
        .append_pair("fields", "first_publish_year")
        .append_pair("limit", "1");
    let body: Docs = get_json_best_effort(config, url.as_str()).await?;
    body.docs
        .into_iter()
        .next()
        .and_then(|d| d.first_publish_year)
}
