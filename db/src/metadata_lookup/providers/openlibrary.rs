//! Open Library provider: the `by_isbn` / `by_title` pair every provider in
//! this directory implements, plus the `enrich` side lookup only this one can
//! serve (no other provider exposes an edition's series statement).
//!
//! Needs no API key, so it is always a reachable rung.

use anyhow::Context;
use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider, ProviderEdition};
use serde::Deserialize;

use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use super::http::{base_url, client, get_json_best_effort, paired_isbn10, sanitize_genres};

/// The `jscmd=data` response is a map keyed `"ISBN:<isbn>"`; a missing key means
/// the ISBN isn't known.
#[derive(Debug, Deserialize)]
struct OlBook {
    /// The edition's own record key (`/books/OL…M`) — the handle a selected
    /// candidate is re-fetched by.
    #[serde(default)]
    key: Option<String>,
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
    /// `jscmd=data` returns subjects as `{name, url}` objects, not strings.
    /// Long and mixed — genre, setting, and character names together — which
    /// is why they go through `sanitize_genres`' cap.
    #[serde(default)]
    subjects: Vec<OlNamed>,
    #[serde(default)]
    identifiers: OlIdentifiers,
}

/// The edition's identifier bag. Only the ISBN-10 list is read: the ISBN-13
/// is the caller's own, already normalized.
#[derive(Debug, Default, Deserialize)]
struct OlIdentifiers {
    #[serde(default)]
    isbn_10: Vec<String>,
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
) -> anyhow::Result<Option<ProviderEdition>> {
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

    Ok(Some(ProviderEdition {
        source: MetadataProvider::OpenLibrary,
        provider_ref: provider_ref(book.key, isbn13),
        isbn13: isbn13.to_string(),
        isbn10: paired_isbn10(&book.identifiers.isbn_10, isbn13),
        title,
        authors: book.authors.into_iter().filter_map(|a| a.name).collect(),
        year: book.publish_date,
        // A record that reports 0 pages is one whose length Open Library
        // doesn't know, not a zero-page book — and staging that over a real
        // count is the failure this guards.
        pages: book.number_of_pages.filter(|p| *p > 0),
        publisher: book.publishers.into_iter().find_map(|p| p.name),
        description: None,
        cover_url,
        series: None,
        first_publish_year: None,
        genres: sanitize_genres(book.subjects.into_iter().flat_map(|s| s.name)),
    }))
}

/// The provider handle for one candidate: Open Library's own record key when
/// the row carries one, else the uniform `isbn:` fallback every provider
/// shares.
fn provider_ref(key: Option<String>, isbn13: &str) -> String {
    key.filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| format!("isbn:{isbn13}"))
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
    /// The work key (`/works/OL…W`) — `search.json` answers works, so this is
    /// the coarsest handle of the three providers'.
    #[serde(default)]
    key: Option<String>,
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
    /// Work-level subject list — the same mixed bag as the edition record's,
    /// and the longest of the three providers'.
    #[serde(default)]
    subject: Vec<String>,
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
            "key,title,author_name,first_publish_year,isbn,cover_i,number_of_pages_median,subject",
        )
        .append_pair("limit", &SEARCH_LIMIT.to_string());
    Ok(url.into())
}

/// Search by title text. `Ok(empty)` on no matches; `Err` on transport/parse
/// failure.
pub async fn by_title(
    config: &MetadataLookupConfig,
    query: &str,
) -> anyhow::Result<Vec<ProviderEdition>> {
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

/// Map one search doc into `ProviderEdition`. A doc without a title or a
/// valid ISBN maps to `None` — the whole check-in flow keys on the ISBN
/// downstream (stored per copy, printed on the confirm screens), so a
/// candidate that can't supply one isn't actionable.
fn map_search_doc(doc: OlSearchDoc) -> Option<ProviderEdition> {
    let title = doc.title.filter(|t| !t.trim().is_empty())?;
    let isbn13 = doc.isbn.iter().find_map(|v| normalize_isbn(v).ok())?;
    // `search.json` answers *works*: one `isbn` list spans every edition, so
    // only the entry that re-derives the ISBN-13 above is known to name the
    // same printing.
    let isbn10 = paired_isbn10(&doc.isbn, &isbn13);
    Some(ProviderEdition {
        source: MetadataProvider::OpenLibrary,
        provider_ref: provider_ref(doc.key, &isbn13),
        isbn13,
        isbn10,
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
        genres: sanitize_genres(doc.subject),
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

// ── Detail record (hydrate-on-select) ────────────────────────────

/// Open Library's description, which is `"text"` on some records and
/// `{"type": "/type/text", "value": "text"}` on others. Untagged so both
/// parse into one shape.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OlDescription {
    Text(String),
    Typed { value: String },
}

impl OlDescription {
    fn into_text(self) -> String {
        match self {
            OlDescription::Text(v) => v,
            OlDescription::Typed { value } => value,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OlRecord {
    #[serde(default)]
    description: Option<OlDescription>,
}

/// Whether `key` is shaped like an Open Library record path this crate is
/// willing to address.
///
/// `key` reaches here from the client, which is echoing back a `provider_ref`
/// the search handed out — but "echoing back" is a claim, not a guarantee, so
/// the shape is checked rather than trusted. Anything else (a bare `isbn:`
/// fallback ref, a path with a `..` segment, an absolute URL) is refused, so
/// the request can only ever address a record under the configured base.
fn is_record_key(key: &str) -> bool {
    (key.starts_with("/works/") || key.starts_with("/books/"))
        && key.len() <= 64
        && !key.contains("..")
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-'))
}

/// The description behind one selected candidate, fetched best-effort from
/// its own record.
///
/// This is the field hydrate-on-select exists for: neither `search.json` nor
/// `api/books?jscmd=data` carries a description, so without this call Open
/// Library can never fill the compare view's largest row.
pub async fn describe(config: &MetadataLookupConfig, key: &str) -> Option<String> {
    if !is_record_key(key) {
        return None;
    }
    let url = base_url(
        &config.openlibrary_base,
        &format!("{key}.json"),
        "open library",
    )
    .ok()?;
    let record: OlRecord = get_json_best_effort(config, url.as_str()).await?;
    // Dropped rather than truncated when oversized: this value is staged into
    // the edit form and posted back to a write path, where
    // `MetadataOverrides::validate` would reject a mangled one.
    record
        .description
        .map(OlDescription::into_text)
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty() && d.chars().count() <= ExternalBookMeta::DESCRIPTION_MAX_LEN)
}
