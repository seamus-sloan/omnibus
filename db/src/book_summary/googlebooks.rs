//! Google Books description fetch for the "fetch summary" action. Queries
//! `volumes?q=isbn:` per ISBN, then falls back to a title + author search, and
//! reads `volumeInfo.description`. The API key (when configured) rides in a
//! header, so no URL here carries a credential for an error to render — see
//! [`API_KEY_HEADER`].

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

/// Per-request timeout. A fetch makes up to `isbns.len() + 1` hops, each bounded
/// by this.
const GB_TIMEOUT: Duration = Duration::from_secs(8);

/// Backoff between retries of a retryable status; length + 1 is the attempt
/// count. Keyless requests share an anonymous quota that answers 429/503 often,
/// so a couple of quick retries meaningfully raise the hit rate.
const GB_RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(600)];

/// The header Google accepts the API key in. Same key and same reasoning as
/// `metadata_lookup::providers::googlebooks::API_KEY_HEADER`: a
/// `reqwest::Error` renders the request URL but never a header, so a key that
/// is never in a URL cannot reach a log from any fallible step.
///
/// This client's decode site called no `without_url` at all, so its error
/// carried the key — but nothing here ever printed it: `fetch_summary` reaches
/// only `%e` sinks, and anyhow's non-alternate `Display` stops at the outer
/// context. That is the whole difference from `metadata_lookup`, whose `{e:#}`
/// warns turned the same omission into a real leak. A latent hole guarded by
/// its callers' choice of format specifier is not one worth keeping.
const API_KEY_HEADER: &str = "X-goog-api-key";

/// Connection config for the Google Books description fetch. `base_url` is
/// injectable so tests point it at a local `wiremock` server; `api_key` is the
/// effective key resolved from settings/env by the caller.
#[derive(Debug, Clone)]
pub struct GoogleBooksSummaryConfig {
    /// `https://www.googleapis.com` in production.
    pub base_url: String,
    /// Optional API key. Without one the API still answers, but on a shared
    /// anonymous quota that a self-hosted instance hits (HTTP 429). Sent as a
    /// header, so no error on this path has it to render.
    pub api_key: Option<String>,
    pub timeout: Duration,
}

impl GoogleBooksSummaryConfig {
    /// Config pointing at the live Google Books endpoint with an
    /// already-resolved key (the caller passes the effective settings/env key).
    pub fn live(api_key: Option<String>) -> Self {
        Self {
            base_url: "https://www.googleapis.com".to_string(),
            api_key,
            timeout: GB_TIMEOUT,
        }
    }
}

impl Default for GoogleBooksSummaryConfig {
    fn default() -> Self {
        Self::live(None)
    }
}

/// Process-wide `reqwest::Client`, built once and cloned so fetches share one
/// connection pool + TLS session cache. Fallible (TLS backend init).
fn client() -> reqwest::Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let new = crate::http_client::build_client(&crate::http_client::default_user_agent())?;
    Ok(CLIENT.get_or_init(|| new).clone())
}

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
    description: Option<String>,
}

/// Fetch a book description from Google Books. Tries each ISBN's `q=isbn:`
/// query first, then a title + author search. `Ok(None)` when nothing matched
/// or the matched volume has no description; `Err` only on transport/parse
/// failure of a request that was actually made.
pub async fn fetch(
    config: &GoogleBooksSummaryConfig,
    isbns: &[String],
    title: &str,
    author: Option<&str>,
) -> anyhow::Result<Option<String>> {
    for isbn in isbns {
        if let Some(desc) = query_description(config, &format!("isbn:{isbn}")).await? {
            return Ok(Some(desc));
        }
    }

    if !title.trim().is_empty() {
        if let Some(desc) = query_description(config, &title_query(title, author)).await? {
            return Ok(Some(desc));
        }
    }

    Ok(None)
}

/// Build the `q=` value for a title + optional author search. Google Books
/// treats space-separated `intitle:`/`inauthor:` terms as an AND.
fn title_query(title: &str, author: Option<&str>) -> String {
    match author.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => format!("intitle:{title} inauthor:{a}"),
        None => format!("intitle:{title}"),
    }
}

/// Run one `volumes?q=…` query and return the first non-blank description found
/// across the returned volumes.
async fn query_description(
    config: &GoogleBooksSummaryConfig,
    q: &str,
) -> anyhow::Result<Option<String>> {
    let resp = get_with_retry(config, q).await?;
    let body: GbResponse = resp
        .json()
        .await
        .map_err(reqwest::Error::without_url)
        .context("google books response was not valid json")?;
    Ok(body
        .items
        .into_iter()
        .filter_map(|i| i.volume_info)
        .filter_map(|v| v.description)
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty()))
}

/// GET the volumes URL for `q`, retrying a retryable status a couple of times.
/// The one place the key is attached, and the reason no builder here appends
/// it. Errors are still stripped of their URL, which is noise in a log.
async fn get_with_retry(
    config: &GoogleBooksSummaryConfig,
    q: &str,
) -> anyhow::Result<reqwest::Response> {
    let url = volumes_url(config, q)?;
    let mut last: Option<reqwest::StatusCode> = None;
    for attempt in 0..=GB_RETRY_BACKOFF.len() {
        let mut request = client()?.get(url.clone()).timeout(config.timeout);
        if let Some(key) = config.api_key.as_deref() {
            request = request.header(API_KEY_HEADER, key);
        }
        let resp = request
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("google books request failed")?;

        if !is_retryable(resp.status()) {
            return resp
                .error_for_status()
                .map_err(reqwest::Error::without_url)
                .context("google books returned an error status");
        }
        last = Some(resp.status());
        if let Some(backoff) = GB_RETRY_BACKOFF.get(attempt) {
            tokio::time::sleep(*backoff).await;
        }
    }
    // Status only: the URL diagnoses nothing the status doesn't.
    anyhow::bail!(
        "google books unavailable after {} attempts (last status {})",
        GB_RETRY_BACKOFF.len() + 1,
        last.map_or(0, |s| s.as_u16())
    )
}

/// Whether a status is worth another attempt: 429 (quota) or any 5xx (Google
/// intermittently answers `503 backendFailed` for valid queries).
fn is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Build the volumes URL, percent-encoding `q`. Carries no key — that rides on
/// [`API_KEY_HEADER`], attached in [`get_with_retry`].
fn volumes_url(config: &GoogleBooksSummaryConfig, q: &str) -> anyhow::Result<reqwest::Url> {
    reqwest::Url::parse_with_params(
        &format!("{}/books/v1/volumes", config.base_url),
        &[("q", q)],
    )
    .context("failed to build google books url")
}
