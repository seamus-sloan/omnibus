//! Open Library cascade resolver. Looks up an author by name, fetches a
//! cover photo from the OL covers endpoint, and persists the result (or a
//! sticky `letter` marker on a clean miss) via [`crate::author_photos_data`].

use std::time::Duration;

use serde::Deserialize;
use sqlx::SqlitePool;

use crate::author_photos::shared::{default_user_agent, shared_client};
use crate::author_photos_data::{
    author_photo_status, delete_author_photo, upsert_author_photo, AuthorPhotoSource,
    AuthorPhotosDataError,
};

/// Default request timeout for the Open Library search + cover calls.
const OPEN_LIBRARY_TIMEOUT: Duration = Duration::from_secs(5);

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
            timeout: OPEN_LIBRARY_TIMEOUT,
            user_agent: default_user_agent(),
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
pub async fn resolve(pool: &SqlitePool, author_id: i64) -> anyhow::Result<()> {
    resolve_with(pool, author_id, &OpenLibraryConfig::default()).await
}

/// [`resolve`] with an injectable [`OpenLibraryConfig`] for tests.
pub async fn resolve_with(
    pool: &SqlitePool,
    author_id: i64,
    config: &OpenLibraryConfig,
) -> anyhow::Result<()> {
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

/// Bulk re-resolve all author photos. Clears non-manual cached photos and
/// re-runs the Open Library cascade for every author. Manual uploads are
/// preserved. Per-author errors are logged and skipped so a single failure
/// does not abort the batch.
pub async fn refetch_all(
    pool: &SqlitePool,
    on_progress: impl Fn(u32, Option<u32>),
) -> Result<(), AuthorPhotosDataError> {
    let author_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM authors ORDER BY id")
        .fetch_all(pool)
        .await?;
    let total = u32::try_from(author_ids.len()).unwrap_or(u32::MAX);

    for (i, author_id) in author_ids.iter().enumerate() {
        let done = u32::try_from(i).unwrap_or(u32::MAX).saturating_add(1);
        match author_photo_status(pool, *author_id).await {
            Ok(Some((AuthorPhotoSource::Manual, _))) => {
                on_progress(done, Some(total));
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(author_id, error = %e, "refetch_all: status check failed, skipping");
                on_progress(done, Some(total));
                continue;
            }
        }
        if let Err(e) = delete_author_photo(pool, *author_id).await {
            tracing::warn!(author_id, error = %e, "refetch_all: delete failed, skipping");
            on_progress(done, Some(total));
            continue;
        }
        if let Err(e) = resolve(pool, *author_id).await {
            tracing::warn!(author_id, error = %e, "refetch_all: resolve failed, continuing");
        }
        on_progress(done, Some(total));
    }
    Ok(())
}

/// Two-step Open Library lookup. Returns `(canonical_url, mime, bytes)` on
/// a hit, `None` on a clean miss, or `Err` on a network / decode error.
pub(super) async fn fetch_open_library(
    name: &str,
    config: &OpenLibraryConfig,
) -> Result<Option<(String, String, Vec<u8>)>, reqwest::Error> {
    let client = shared_client()?;

    // Step 1: search for an OLID by name.
    let search_url = format!(
        "{}/search/authors.json?q={}",
        config.base_search_url.trim_end_matches('/'),
        urlencoding::encode(name)
    );
    let resp = client
        .get(&search_url)
        .timeout(config.timeout)
        .header(reqwest::header::USER_AGENT, &config.user_agent)
        .send()
        .await?;
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
    let resp = client
        .get(&cover_url)
        .timeout(config.timeout)
        .header(reqwest::header::USER_AGENT, &config.user_agent)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
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
