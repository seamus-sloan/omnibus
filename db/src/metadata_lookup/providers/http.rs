//! HTTP plumbing shared by the provider clients: the process-wide client, the
//! URL-stripping every provider error goes through, and the small response
//! helpers more than one provider needs.

use std::sync::OnceLock;

use anyhow::Context;

use super::super::MetadataLookupConfig;

/// Process-wide `reqwest::Client`, built once and cloned so lookups share one
/// connection pool + TLS session cache. Fallible (TLS backend init).
pub(super) fn client() -> reqwest::Result<reqwest::Client> {
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
pub(super) fn strip_url(e: reqwest::Error) -> reqwest::Error {
    e.without_url()
}

/// GET + parse JSON, degrading every failure to `None` with a debug log. For
/// best-effort side lookups (enrichment) that must never fail their caller.
pub(super) async fn get_json_best_effort<T: serde::de::DeserializeOwned>(
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
            tracing::debug!("provider side-lookup miss: {e:#}");
            None
        }
    }
}

/// Parse a provider base URL, so a malformed one is a clear error rather than
/// a request to nowhere.
pub(super) fn base_url(base: &str, path: &str, provider: &str) -> anyhow::Result<reqwest::Url> {
    reqwest::Url::parse(&format!("{base}{path}"))
        .with_context(|| format!("invalid {provider} base url"))
}

/// Reduce a publication date to its year.
///
/// Google Books returns `publishedDate` in whatever precision it holds —
/// `"2025"`, `"2025-02"`, or `"2025-02-25"` — while Open Library gives a bare
/// year. The chooser card renders this next to the title ("Dune · 2005"), so
/// an un-trimmed value makes two providers look like two different fields.
/// Anything that doesn't start with four digits is passed through untouched
/// rather than guessed at.
pub(crate) fn publication_year(raw: &str) -> Option<String> {
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

/// Upgrade an `http://` URL to `https://`; other schemes pass through
/// unchanged. Google Books returns cover links over plain HTTP, which a
/// browser blocks as mixed content on an HTTPS page.
pub(super) fn upgrade_to_https(url: &str) -> String {
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
