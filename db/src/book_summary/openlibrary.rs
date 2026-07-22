//! OpenLibrary description fetch for the "fetch summary" action. Unlike
//! [`crate::metadata_lookup`] (which hits `jscmd=data`, a view that carries no
//! description), a book's blurb lives on its *work*, so this resolves the work
//! key first — by ISBN (`/isbn/{isbn}.json`) or, failing that, by a title +
//! author search — then reads `description` off `/works/{OLID}.json`.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

/// Per-request timeout. A fetch makes up to two hops (edition → work), each
/// bounded by this.
const OL_TIMEOUT: Duration = Duration::from_secs(8);

/// Connection config for the OpenLibrary description fetch. `base_url` is
/// injectable so tests point it at a local `wiremock` server.
#[derive(Debug, Clone)]
pub struct OpenLibrarySummaryConfig {
    /// `https://openlibrary.org` in production.
    pub base_url: String,
    pub timeout: Duration,
}

impl OpenLibrarySummaryConfig {
    /// Config pointing at the live OpenLibrary endpoint.
    pub fn live() -> Self {
        Self {
            base_url: "https://openlibrary.org".to_string(),
            timeout: OL_TIMEOUT,
        }
    }
}

impl Default for OpenLibrarySummaryConfig {
    fn default() -> Self {
        Self::live()
    }
}

/// Process-wide `reqwest::Client`, built once and cloned so fetches share one
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

/// A work reference on an edition record, e.g. `{"key": "/works/OL45804W"}`.
#[derive(Debug, Deserialize)]
struct WorkRef {
    #[serde(default)]
    key: Option<String>,
}

/// The subset of `/isbn/{isbn}.json` we read — just the work links.
#[derive(Debug, Deserialize)]
struct EditionRecord {
    #[serde(default)]
    works: Vec<WorkRef>,
}

/// A `/search.json` hit; `key` is the work key (`/works/OL…W`).
#[derive(Debug, Deserialize)]
struct SearchDoc {
    #[serde(default)]
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    docs: Vec<SearchDoc>,
}

/// `/works/{OLID}.json`'s `description`, which OpenLibrary returns either as a
/// bare string or as a `{ "type": "/type/text", "value": "…" }` object.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OlDescription {
    Text(String),
    Rich { value: String },
}

#[derive(Debug, Deserialize)]
struct WorkRecord {
    #[serde(default)]
    description: Option<OlDescription>,
}

/// Fetch a book description from OpenLibrary. Tries each ISBN's edition→work
/// chain first, then a title + author search. `Ok(None)` when nothing matched
/// or the resolved work has no description; `Err` only on transport/parse
/// failure of a request that was actually made.
pub async fn fetch(
    config: &OpenLibrarySummaryConfig,
    isbns: &[String],
    title: &str,
    author: Option<&str>,
) -> anyhow::Result<Option<String>> {
    for isbn in isbns {
        if let Some(work_key) = work_key_from_isbn(config, isbn).await? {
            if let Some(desc) = description_from_work(config, &work_key).await? {
                return Ok(Some(desc));
            }
        }
    }

    if !title.trim().is_empty() {
        if let Some(work_key) = work_key_from_search(config, title, author).await? {
            if let Some(desc) = description_from_work(config, &work_key).await? {
                return Ok(Some(desc));
            }
        }
    }

    Ok(None)
}

/// Resolve an ISBN to its work key via `/isbn/{isbn}.json`. A 404 (unknown
/// ISBN) is a clean miss (`Ok(None)`), not an error.
async fn work_key_from_isbn(
    config: &OpenLibrarySummaryConfig,
    isbn: &str,
) -> anyhow::Result<Option<String>> {
    let url = format!("{}/isbn/{isbn}.json", config.base_url);
    let resp = client()?
        .get(&url)
        .timeout(config.timeout)
        .send()
        .await
        .context("open library isbn request failed")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let edition: EditionRecord = resp
        .error_for_status()
        .context("open library isbn lookup returned an error status")?
        .json()
        .await
        .context("open library isbn response was not valid json")?;
    Ok(edition.works.into_iter().find_map(|w| w.key))
}

/// Resolve a title + optional author to a work key via `/search.json`.
async fn work_key_from_search(
    config: &OpenLibrarySummaryConfig,
    title: &str,
    author: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let mut query: Vec<(&str, &str)> = vec![("title", title), ("limit", "1"), ("fields", "key")];
    if let Some(a) = author.filter(|a| !a.trim().is_empty()) {
        query.push(("author", a));
    }
    // `parse_with_params` percent-encodes the title/author values.
    let url = reqwest::Url::parse_with_params(&format!("{}/search.json", config.base_url), &query)
        .context("failed to build open library search url")?;
    let resp: SearchResponse = client()?
        .get(url)
        .timeout(config.timeout)
        .send()
        .await
        .context("open library search request failed")?
        .error_for_status()
        .context("open library search returned an error status")?
        .json()
        .await
        .context("open library search response was not valid json")?;
    Ok(resp.docs.into_iter().find_map(|d| d.key))
}

/// Read a work's `description`, normalizing the string-or-object shape and
/// treating blank text as absent.
async fn description_from_work(
    config: &OpenLibrarySummaryConfig,
    work_key: &str,
) -> anyhow::Result<Option<String>> {
    // `work_key` is already `/works/OL…W`; join without doubling the slash.
    let url = format!("{}{work_key}.json", config.base_url);
    let record: WorkRecord = client()?
        .get(&url)
        .timeout(config.timeout)
        .send()
        .await
        .context("open library work request failed")?
        .error_for_status()
        .context("open library work lookup returned an error status")?
        .json()
        .await
        .context("open library work response was not valid json")?;
    Ok(record
        .description
        .map(|d| match d {
            OlDescription::Text(s) => s,
            OlDescription::Rich { value } => value,
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}
