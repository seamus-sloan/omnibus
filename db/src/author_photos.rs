//! F1.11 Author profile photo resolver.
//!
//! Runs the manual → Open Library → letter cascade for a given author and
//! persists the result in `author_photos`. Driven by
//! [`crate::worker::Task::ResolveAuthorPhoto`], which gives us:
//!   - at most one in-flight resolution per author (per-resource keyed
//!     mutex),
//!   - background execution, so the page renders the letter avatar
//!     immediately and upgrades on the next visit.
//!
//! Cache semantics: a `'letter'` row is written on any miss (network
//! error, Open Library has no match, sub-1KB image, non-image bytes) and
//! sticks until an admin clears it via `DELETE /api/authors/:id/photo`.
//! This keeps a single page-view from costing two HTTP round-trips on
//! every refresh for authors who genuinely have no Open Library entry.

use std::time::Duration;

use serde::Deserialize;
use sqlx::SqlitePool;

use crate::queries::{author_photo_status, upsert_author_photo, AuthorPhotoSource};

/// Injection points for tests. Production builds construct `default()` and
/// hit the real Open Library endpoints.
#[derive(Debug, Clone)]
pub struct OpenLibraryConfig {
    /// `https://openlibrary.org` in production. Tests point this at a
    /// `wiremock` server.
    pub base_search_url: String,
    /// `https://covers.openlibrary.org` in production. Separate from
    /// `base_search_url` because Open Library serves search and covers on
    /// different hostnames.
    pub base_covers_url: String,
    pub timeout: Duration,
    pub user_agent: String,
}

impl Default for OpenLibraryConfig {
    fn default() -> Self {
        Self {
            base_search_url: "https://openlibrary.org".into(),
            base_covers_url: "https://covers.openlibrary.org".into(),
            timeout: Duration::from_secs(5),
            user_agent: format!(
                "omnibus/{} (https://github.com/sloansa/omnibus)",
                env!("CARGO_PKG_VERSION")
            ),
        }
    }
}

/// Minimum image size in bytes that we'll accept as a real photo. Open
/// Library returns a tiny placeholder when an OLID has no cover and we
/// forgot to set `default=false` — guard against the case anyway.
const MIN_IMAGE_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
struct AuthorSearchResponse {
    #[serde(default)]
    docs: Vec<AuthorSearchHit>,
}

#[derive(Debug, Deserialize)]
struct AuthorSearchHit {
    /// e.g. `"OL23919A"` — Open Library author id.
    #[serde(default)]
    key: Option<String>,
}

/// Resolve and persist a profile photo for `author_id`.
///
/// Cascade:
///   1. If a non-letter row already exists, no-op (the admin upload or
///      previous resolution wins).
///   2. If a `letter` row exists, no-op (sticky negative cache).
///   3. Look up the author's name and query Open Library.
///   4. On a hit: persist an `openlibrary` row with bytes.
///      On a clean miss (search returned no docs, cover endpoint 404,
///      image smaller than [`MIN_IMAGE_BYTES`], non-image MIME): write a
///      sticky `letter` marker.
///      On a transient error (network failure, JSON decode error, etc.):
///      leave the row absent so the next page view can retry. Otherwise a
///      single Open Library outage would permanently brick resolution for
///      that author until an admin cleared it.
pub async fn resolve(pool: &SqlitePool, author_id: i64) -> Result<(), sqlx::Error> {
    resolve_with(pool, author_id, &OpenLibraryConfig::default()).await
}

pub async fn resolve_with(
    pool: &SqlitePool,
    author_id: i64,
    config: &OpenLibraryConfig,
) -> Result<(), sqlx::Error> {
    // Existing row wins. `'letter'` markers are sticky so we don't re-hit
    // Open Library for authors with no entry on every page view.
    if author_photo_status(pool, author_id).await?.is_some() {
        return Ok(());
    }

    let name: Option<String> = sqlx::query_scalar("SELECT name FROM authors WHERE id = ?")
        .bind(author_id)
        .fetch_optional(pool)
        .await?;
    let Some(name) = name else {
        return Ok(());
    };

    match fetch_open_library(&name, config).await {
        Ok(Some((url, mime, bytes))) => {
            upsert_author_photo(
                pool,
                author_id,
                AuthorPhotoSource::OpenLibrary,
                Some(&url),
                Some(&mime),
                Some(&bytes),
            )
            .await?;
        }
        Ok(None) => {
            // Clean miss — Open Library has nothing for this name. Record
            // a sticky `letter` marker so we don't re-query on every view.
            upsert_author_photo(pool, author_id, AuthorPhotoSource::Letter, None, None, None)
                .await?;
        }
        Err(e) => {
            // Transient network / decode failure — leave the row absent so a
            // later page view (or scan) can retry. Logged so an outage is
            // still visible in the worker log.
            tracing::warn!(
                author_id,
                error = %e,
                "open library resolution failed (transient); leaving row absent for retry"
            );
        }
    }
    Ok(())
}

/// Two-step Open Library lookup. Returns `(canonical_url, mime, bytes)` on
/// a hit, `None` on a clean miss, or `Err` on a network / decode error.
async fn fetch_open_library(
    name: &str,
    config: &OpenLibraryConfig,
) -> Result<Option<(String, String, Vec<u8>)>, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .user_agent(&config.user_agent)
        .build()?;

    // Step 1: search for an OLID by name.
    let search_url = format!(
        "{}/search/authors.json?q={}",
        config.base_search_url.trim_end_matches('/'),
        urlencoding::encode(name)
    );
    let resp = client.get(&search_url).send().await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let body: AuthorSearchResponse = resp.json().await?;
    let Some(olid) = body
        .docs
        .into_iter()
        .find_map(|d| d.key.filter(|k| !k.is_empty()))
    else {
        return Ok(None);
    };

    // Step 2: fetch the cover. `default=false` makes Open Library return a
    // 404 instead of a 1x1 placeholder when the OLID has no photo.
    let cover_url = format!(
        "{}/a/olid/{}-L.jpg?default=false",
        config.base_covers_url.trim_end_matches('/'),
        olid
    );
    let resp = client.get(&cover_url).send().await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "image/jpeg".into());
    if !content_type.starts_with("image/") {
        return Ok(None);
    }
    let bytes = resp.bytes().await?.to_vec();
    if bytes.len() < MIN_IMAGE_BYTES {
        return Ok(None);
    }
    Ok(Some((cover_url, content_type, bytes)))
}

/// Hard cap on the bytes we'll read from a user-supplied URL. Same 10 MiB
/// budget as the multipart upload route — callers should pre-check
/// Content-Length when available, but this cap is what actually bounds
/// memory consumption mid-download.
pub const REMOTE_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Errors surfaced by [`fetch_remote_image`]. The variants are deliberately
/// user-facing — the handler maps each to a 4xx/5xx response without further
/// rephrasing.
#[derive(Debug, thiserror::Error)]
pub enum FetchRemoteImageError {
    #[error("URL must start with http:// or https://")]
    BadScheme,
    #[error("remote server returned {0}")]
    BadStatus(u16),
    #[error("remote response content-type is not an image ({0})")]
    NotImage(String),
    #[error("SVG photos are not accepted")]
    SvgRejected,
    #[error("image exceeds {} byte cap", REMOTE_IMAGE_MAX_BYTES)]
    TooLarge,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// Fetch an image from a user-supplied URL with the same validation gates as
/// the multipart upload route — image content-type, no SVG, size cap. Returns
/// the raw bytes and the server-advertised content-type; callers are
/// expected to run magic-byte sniffing on the bytes before persisting.
///
/// Used by the "paste image URL" branch of the author photo edit modal
/// (F1.11 follow-up). Lives next to [`fetch_open_library`] because it
/// reuses the same `reqwest` setup and shares the "we expect an image"
/// surface area.
pub async fn fetch_remote_image(url: &str) -> Result<(String, Vec<u8>), FetchRemoteImageError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(FetchRemoteImageError::BadScheme);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!(
            "omnibus/{} (https://github.com/sloansa/omnibus)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(FetchRemoteImageError::BadStatus(status.as_u16()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "application/octet-stream".into());
    if !content_type.starts_with("image/") {
        return Err(FetchRemoteImageError::NotImage(content_type));
    }
    if content_type.contains("svg") {
        return Err(FetchRemoteImageError::SvgRejected);
    }
    // Pre-check Content-Length when the server advertises it so an
    // obviously-oversized download bails before allocating.
    if let Some(len) = resp.content_length() {
        if len > REMOTE_IMAGE_MAX_BYTES {
            return Err(FetchRemoteImageError::TooLarge);
        }
    }
    let bytes = resp.bytes().await?.to_vec();
    if bytes.len() as u64 > REMOTE_IMAGE_MAX_BYTES {
        return Err(FetchRemoteImageError::TooLarge);
    }
    Ok((content_type, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{get_author_photo, init_db};
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    async fn pool_with_author(name: &str) -> (SqlitePool, i64) {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let id: i64 =
            sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
                .bind(name)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        (pool, id)
    }

    fn config_for(server: &MockServer) -> OpenLibraryConfig {
        OpenLibraryConfig {
            base_search_url: server.uri(),
            base_covers_url: server.uri(),
            timeout: Duration::from_secs(2),
            user_agent: "omnibus-test".into(),
        }
    }

    #[tokio::test]
    async fn resolve_writes_open_library_hit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/authors.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "docs": [ { "key": "OL23919A" } ]
            })))
            .mount(&server)
            .await;
        // 2 KB of bytes so the MIN_IMAGE_BYTES guard passes.
        let payload = vec![0xABu8; 2048];
        Mock::given(method("GET"))
            .and(path("/a/olid/OL23919A-L.jpg"))
            .and(query_param("default", "false"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(payload.clone()),
            )
            .mount(&server)
            .await;

        let (pool, id) = pool_with_author("Ada Lovelace").await;
        let cfg = config_for(&server);
        resolve_with(&pool, id, &cfg).await.unwrap();

        let (mime, bytes) = get_author_photo(&pool, id).await.unwrap().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(bytes, payload);
        let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::OpenLibrary);
    }

    #[tokio::test]
    async fn resolve_writes_letter_when_search_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/authors.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "docs": []
            })))
            .mount(&server)
            .await;

        let (pool, id) = pool_with_author("Nobody In Particular").await;
        let cfg = config_for(&server);
        resolve_with(&pool, id, &cfg).await.unwrap();

        assert!(get_author_photo(&pool, id).await.unwrap().is_none());
        let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }

    #[tokio::test]
    async fn resolve_writes_letter_when_cover_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/authors.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "docs": [ { "key": "OL999A" } ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/a/olid/OL999A-L.jpg"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let (pool, id) = pool_with_author("Ada Lovelace").await;
        let cfg = config_for(&server);
        resolve_with(&pool, id, &cfg).await.unwrap();

        let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }

    #[tokio::test]
    async fn resolve_writes_letter_when_image_too_small() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/authors.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "docs": [ { "key": "OL1A" } ]
            })))
            .mount(&server)
            .await;
        // Tiny placeholder (well under MIN_IMAGE_BYTES) — should be treated
        // as a miss.
        Mock::given(method("GET"))
            .and(path("/a/olid/OL1A-L.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/gif")
                    .set_body_bytes(vec![0u8; 42]),
            )
            .mount(&server)
            .await;

        let (pool, id) = pool_with_author("Ada Lovelace").await;
        let cfg = config_for(&server);
        resolve_with(&pool, id, &cfg).await.unwrap();

        let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }

    #[tokio::test]
    async fn resolve_is_noop_when_letter_marker_exists() {
        // Existing letter marker must prevent any HTTP call. We assert this
        // by starting a mock server with *no* mounted responses — any
        // incoming request would 404 and we'd notice via the marker source
        // not changing.
        let server = MockServer::start().await;
        let (pool, id) = pool_with_author("Ada Lovelace").await;
        upsert_author_photo(&pool, id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let cfg = config_for(&server);
        resolve_with(&pool, id, &cfg).await.unwrap();

        let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            0,
            "letter marker must skip the network entirely"
        );
    }

    #[tokio::test]
    async fn resolve_is_noop_when_manual_upload_exists() {
        let server = MockServer::start().await;
        let (pool, id) = pool_with_author("Ada Lovelace").await;
        upsert_author_photo(
            &pool,
            id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFmanual"),
        )
        .await
        .unwrap();
        let cfg = config_for(&server);
        resolve_with(&pool, id, &cfg).await.unwrap();

        let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Manual);
    }

    #[tokio::test]
    async fn resolve_leaves_row_absent_on_transient_network_error() {
        // Point the resolver at a TCP port that nothing is listening on so
        // every request errors at the transport layer. A transient outage
        // must NOT cache a `letter` marker — the next call should be free
        // to retry, not stuck for an admin to manually clear.
        let cfg = OpenLibraryConfig {
            base_search_url: "http://127.0.0.1:1".into(),
            base_covers_url: "http://127.0.0.1:1".into(),
            timeout: Duration::from_millis(500),
            user_agent: "omnibus-test".into(),
        };
        let (pool, id) = pool_with_author("Ada Lovelace").await;
        resolve_with(&pool, id, &cfg).await.unwrap();

        assert!(
            author_photo_status(&pool, id).await.unwrap().is_none(),
            "transient network error must leave the row absent for retry"
        );
    }
}
