//! Open Library provider: the `by_isbn` / `by_title` pair every provider in
//! this directory implements, plus the `enrich` side lookup only this one can
//! serve (no other provider exposes an edition's series statement).
//!
//! Needs no API key, so it is always a reachable rung.

use anyhow::Context;
use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider, ProviderEdition};
use serde::Deserialize;

use super::super::query::SearchQuery;
use super::super::{MetadataLookupConfig, SEARCH_LIMIT};
use super::http::{
    base_url, client, get_json_best_effort, paired_isbn10, publication_year, sanitize_genres,
};

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
        // The ISBN path always has an ISBN, so the handle always resolves.
        provider_ref: provider_ref(book.key, Some(isbn13))
            .unwrap_or_else(|| format!("isbn:{isbn13}")),
        isbn13: Some(isbn13.to_string()),
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
        series_index: None,
        first_publish_year: None,
        genres: sanitize_genres(book.subjects.into_iter().flat_map(|s| s.name)),
        relevance: None,
    }))
}

/// The provider handle for one candidate: Open Library's own record key when
/// the row carries one, else the uniform `isbn:` fallback every provider
/// shares. `None` when the row has neither, which leaves nothing to re-fetch
/// the candidate by and so nothing worth showing.
fn provider_ref(key: Option<String>, isbn13: Option<&str>) -> Option<String> {
    key.filter(|k| !k.trim().is_empty())
        .or_else(|| isbn13.map(|isbn| format!("isbn:{isbn}")))
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
/// Build the search URL for a structured query.
///
/// `title` and `author` are **separate parameters**, which is the whole point:
/// Open Library matches `title=` against the title field alone, so handing it
/// "Dune Frank Herbert" asks for a book whose *title* contains the author's
/// name — and duly returns "The secrets of Frank Herbert's Dune" and four
/// more books about the novel, with the novel itself nowhere in the page.
pub(in crate::metadata_lookup) fn search_url(
    config: &MetadataLookupConfig,
    query: &SearchQuery,
) -> anyhow::Result<String> {
    let mut url = base_url(&config.openlibrary_base, "/search.json", "open library")?;
    // An ISBN identifies the edition outright, so it is asked on its own —
    // pairing it with a title can only narrow an already-exact answer.
    if let Some(isbn13) = query.isbn13.as_deref() {
        url.query_pairs_mut().append_pair("isbn", isbn13);
    } else {
        url.query_pairs_mut()
            .append_pair("title", query.title.as_deref().unwrap_or_default());
        if let Some(author) = query.author.as_deref() {
            url.query_pairs_mut().append_pair("author", author);
        }
    }
    url.query_pairs_mut()
        .append_pair(
            "fields",
            "key,title,author_name,first_publish_year,isbn,cover_i,number_of_pages_median,subject",
        )
        .append_pair("limit", &SEARCH_LIMIT.to_string());
    Ok(url.into())
}

/// Search. `Ok(empty)` on no matches; `Err` on transport/parse failure.
pub async fn search(
    config: &MetadataLookupConfig,
    query: &SearchQuery,
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

/// Map one search doc into `ProviderEdition`. A doc without a title, or with
/// no handle to re-fetch it by, maps to `None`.
///
/// An ISBN is **not** required. `search.json` answers with *works*, and a work
/// that predates the ISBN — or whose editions Open Library has not catalogued
/// with one — is still a book a reader is editing; requiring one here dropped
/// those rows silently. What is required is the work key, which is what the
/// hydrate step re-asks by.
fn map_search_doc(doc: OlSearchDoc) -> Option<ProviderEdition> {
    let title = doc.title.filter(|t| !t.trim().is_empty())?;
    let isbn13 = doc.isbn.iter().find_map(|v| normalize_isbn(v).ok());
    // `search.json` answers *works*: one `isbn` list spans every edition, so
    // only the entry that re-derives the ISBN-13 above is known to name the
    // same printing.
    let isbn10 = isbn13
        .as_deref()
        .and_then(|i13| paired_isbn10(&doc.isbn, i13));
    Some(ProviderEdition {
        source: MetadataProvider::OpenLibrary,
        provider_ref: provider_ref(doc.key, isbn13.as_deref())?,
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
        series_index: None,
        first_publish_year: doc.first_publish_year,
        genres: sanitize_genres(doc.subject),
        relevance: None,
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

/// An Open Library work record, as `by_ref` reads it. Authors are absent by
/// design — the record carries only `/authors/OL…A` references.
#[derive(Debug, Deserialize)]
struct OlWork {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<OlDescription>,
    #[serde(default)]
    subjects: Vec<String>,
    #[serde(default)]
    covers: Vec<i64>,
    #[serde(default)]
    first_publish_date: Option<String>,
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

/// Re-fetch one candidate by its Open Library record key.
///
/// The hydrate path for a candidate with no ISBN. `Ok(None)` when the key
/// isn't a record path this crate will address (see [`is_record_key`], which
/// is the SSRF guard — the key is a claim from the client, not a fact) or the
/// record has no title.
///
/// A work record carries no authors, only author *references*, and resolving
/// those would be one extra request each for a field the search hit already
/// has — so they are left empty and `fill_missing_from` restores them.
pub async fn by_ref(
    config: &MetadataLookupConfig,
    key: &str,
) -> anyhow::Result<Option<ProviderEdition>> {
    if !is_record_key(key) {
        return Ok(None);
    }
    let url = base_url(
        &config.openlibrary_base,
        &format!("{key}.json"),
        "open library",
    )?;
    let Some(record) = get_json_best_effort::<OlWork>(config, url.as_str()).await else {
        return Ok(None);
    };
    let Some(title) = record.title.filter(|t| !t.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(ProviderEdition {
        source: MetadataProvider::OpenLibrary,
        provider_ref: key.to_string(),
        isbn13: None,
        isbn10: None,
        title,
        authors: Vec::new(),
        year: None,
        pages: None,
        publisher: None,
        description: record
            .description
            .map(OlDescription::into_text)
            .map(|d| d.trim().to_string())
            .filter(|d| {
                !d.is_empty() && d.chars().count() <= ExternalBookMeta::DESCRIPTION_MAX_LEN
            }),
        cover_url: record
            .covers
            .into_iter()
            .find(|id| *id > 0)
            .map(|id| format!("https://covers.openlibrary.org/b/id/{id}-L.jpg")),
        series: None,
        series_index: None,
        first_publish_year: record
            .first_publish_date
            .as_deref()
            .and_then(publication_year)
            .and_then(|y| y.parse().ok()),
        genres: sanitize_genres(record.subjects),
        relevance: None,
    }))
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

#[cfg(test)]
mod tests {
    use super::is_record_key;

    // `base_url` concatenates rather than joining, so what reaches `Url::parse`
    // is `{base}{key}.json`. That makes `is_record_key` the only thing standing
    // between a client-supplied `provider_ref` and a request of its choosing —
    // these cases are the ones that would otherwise get through.

    #[test]
    fn is_record_key_accepts_the_two_record_paths_open_library_actually_mints() {
        assert!(is_record_key("/works/OL893414W"));
        assert!(is_record_key("/books/OL7353617M"));
        assert!(is_record_key("/works/OL_1-2W"));
    }

    #[test]
    fn is_record_key_rejects_the_isbn_fallback_ref_every_provider_shares() {
        // Not a path at all: concatenated onto the base it addresses
        // `https://openlibrary.orgisbn:978….json`, which is not a record.
        assert!(!is_record_key("isbn:9780134685991"));
    }

    #[test]
    fn is_record_key_rejects_a_traversal_segment() {
        // `/works/../../../some/other/path.json` parses to `/some/other/path.json`
        // — a different endpoint on the same host.
        assert!(!is_record_key("/works/../../../some/other/path"));
        assert!(!is_record_key("/books/OL1M/.."));
    }

    #[test]
    fn is_record_key_rejects_a_ref_that_rewrites_the_host() {
        // The dangerous shape: `{base}@evil.example/x.json` parses with
        // `openlibrary.org…` as *userinfo* and `evil.example` as the host, so a
        // ref carrying `@`, a scheme, or `//` must never reach the URL.
        assert!(!is_record_key("@evil.example/x"));
        assert!(!is_record_key("/works/OL1W@evil.example/x"));
        assert!(!is_record_key("https://evil.example/x"));
        assert!(!is_record_key("//evil.example/x"));
        assert!(!is_record_key("/works/OL1W?x=y"));
        assert!(!is_record_key("/works/OL1W#frag"));
    }

    #[test]
    fn is_record_key_rejects_an_over_long_ref() {
        // `EditionHydrateRequest::validate` caps at 256 bytes, four times this
        // guard's own limit, so the length check has to hold on its own.
        let long = format!("/works/{}", "O".repeat(64));
        assert!(long.len() > 64);
        assert!(!is_record_key(&long));
    }

    #[test]
    fn is_record_key_rejects_a_path_outside_the_two_record_prefixes() {
        assert!(!is_record_key("/search.json"));
        assert!(!is_record_key("/authors/OL1A"));
        assert!(!is_record_key(""));
    }
}
