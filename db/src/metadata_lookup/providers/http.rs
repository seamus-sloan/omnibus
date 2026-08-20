//! HTTP plumbing shared by the provider clients: the process-wide client, the
//! URL-stripping every provider error goes through, and the small response
//! helpers more than one provider needs — including the normalizers that keep
//! three differently-shaped APIs answering with one set of rules.

use std::sync::OnceLock;

use anyhow::Context;
use omnibus_shared::ebook::MetadataOverrides;
use omnibus_shared::isbn::normalize_isbn;
use omnibus_shared::metadata_lookup::ProviderEdition;

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

/// Clean one provider's genre labels into what a chip editor can show, capped
/// at [`ProviderEdition::MAX_GENRES`].
///
/// Two rules callers rely on: an over-long label is **dropped, not
/// truncated** — it is posted back to a write path where `validate` would
/// reject a mangled one — and order is preserved, since Open Library returns
/// its subjects roughly most-relevant first, so the cap keeps the useful head.
pub(super) fn sanitize_genres<I>(raw: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for label in raw {
        let label = label.as_ref().trim();
        if label.is_empty() || label.chars().count() > MetadataOverrides::GENRE_MAX_LEN {
            continue;
        }
        let key = label.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(label.to_string());
        if out.len() == ProviderEdition::MAX_GENRES {
            break;
        }
    }
    out
}

/// The ISBN-10 among `candidates` that names the **same printing** as
/// `isbn13`, canonicalized (separators stripped, `x` check digit upper-cased)
/// so it satisfies `MetadataOverrides::validate` as-is.
///
/// Surprising on purpose: a perfectly valid ISBN-10 yields `None` unless it
/// re-derives `isbn13`. Providers list identifiers per *work* as readily as
/// per edition, so an unpaired one names a different printing. A 979-prefixed
/// ISBN-13 pairs with nothing — it has no ISBN-10 form.
pub(super) fn paired_isbn10<I>(candidates: I, isbn13: &str) -> Option<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    candidates.into_iter().find_map(|raw| {
        let canonical = canonical_isbn10(raw.as_ref())?;
        (normalize_isbn(&canonical).ok()? == isbn13).then_some(canonical)
    })
}

/// Strip separators and upper-case the check digit, keeping only a string that
/// is *shaped* like an ISBN-10 — 10 ASCII characters. The check digit itself
/// is verified by [`paired_isbn10`]'s round trip through `normalize_isbn`.
fn canonical_isbn10(raw: &str) -> Option<String> {
    let stripped: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| if c == 'x' { 'X' } else { c })
        .collect();
    (stripped.is_ascii() && stripped.len() == 10).then_some(stripped)
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
    use super::{paired_isbn10, sanitize_genres, upgrade_to_https};
    use omnibus_shared::ebook::MetadataOverrides;
    use omnibus_shared::metadata_lookup::ProviderEdition;

    // Effective Java: the ISBN-13 and the ISBN-10 of the same printing.
    const ISBN13: &str = "9780134685991";
    const ISBN10: &str = "0134685997";

    #[test]
    fn sanitize_genres_trims_dedupes_and_preserves_order() {
        let cleaned = sanitize_genres(["  Fantasy  ", "", "   ", "fantasy", "Science Fiction"]);
        assert_eq!(cleaned, vec!["Fantasy", "Science Fiction"]);
    }

    #[test]
    fn sanitize_genres_caps_the_list_at_the_provider_budget() {
        let many: Vec<String> = (0..40).map(|i| format!("Subject {i}")).collect();
        let cleaned = sanitize_genres(&many);
        assert_eq!(cleaned.len(), ProviderEdition::MAX_GENRES);
        // Order-preserving: the cap keeps the head, not an arbitrary slice.
        assert_eq!(cleaned[0], "Subject 0");
        assert_eq!(
            cleaned[ProviderEdition::MAX_GENRES - 1],
            format!("Subject {}", ProviderEdition::MAX_GENRES - 1)
        );
    }

    #[test]
    fn sanitize_genres_drops_a_label_over_the_stored_cap() {
        // Dropped rather than truncated: the value is posted back to a write
        // path where `MetadataOverrides::validate` would reject a mangled one.
        let oversized = "x".repeat(MetadataOverrides::GENRE_MAX_LEN + 1);
        assert_eq!(
            sanitize_genres([oversized.as_str(), "Fantasy"]),
            vec!["Fantasy"]
        );
    }

    #[test]
    fn paired_isbn10_returns_the_identifier_for_the_same_printing() {
        assert_eq!(
            paired_isbn10(["not-an-isbn", ISBN10, ISBN13], ISBN13),
            Some(ISBN10.to_string())
        );
    }

    #[test]
    fn paired_isbn10_canonicalizes_separators_and_a_lowercase_check_digit() {
        // Sanity: 007462542X is a valid ISBN-10 with an `X` check digit.
        assert_eq!(
            paired_isbn10(["0-07-46254-2x"], "9780074625422"),
            Some("007462542X".to_string())
        );
    }

    #[test]
    fn paired_isbn10_rejects_an_identifier_from_a_different_printing() {
        // A work-level list mixes editions; an ISBN-10 that doesn't re-derive
        // the returned ISBN-13 describes a different book.
        assert_eq!(paired_isbn10([ISBN10], "9780141439518"), None);
    }

    #[test]
    fn paired_isbn10_is_none_for_a_979_isbn13_which_has_no_isbn10_form() {
        assert_eq!(paired_isbn10([ISBN10, ISBN13], "9791234567896"), None);
    }

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
