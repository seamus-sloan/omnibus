//! Author profile photo resolver. Runs the manual → Open Library → letter
//! cascade for a given author and persists the result in `author_photos`.
//! Driven by [`crate::worker::Task::ResolveAuthorPhoto`] so resolutions
//! run in the background with at most one in-flight per author. Any miss
//! writes a sticky `'letter'` row so subsequent page loads don't pay the
//! network round-trip again until an admin clears it.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use sqlx::SqlitePool;

use crate::author_photos_data::{author_photo_status, upsert_author_photo, AuthorPhotoSource};

/// Default request timeout for the Open Library search + cover calls.
const OPEN_LIBRARY_TIMEOUT: Duration = Duration::from_secs(5);

fn default_user_agent() -> String {
    format!(
        "omnibus/{} (https://github.com/sloansa/omnibus)",
        env!("CARGO_PKG_VERSION")
    )
}

/// Process-wide shared `reqwest::Client`.
///
/// `reqwest::Client` is designed to be cloned and reused: a clone shares the
/// underlying connection pool and TLS session cache. Building one per call
/// (the previous behaviour) paid a fresh TLS handshake on every request and
/// reused no connections across the search + cover-download pair, or across
/// the many author-photo resolutions a deployment fires in sequence.
///
/// We hand out clones of this single client so all outbound HTTP in
/// `db::author_photos` — the Worker's cascade resolutions *and* the admin
/// "paste image URL" endpoint — shares one pool. The per-request timeout
/// differs between call sites, so it's applied on the `RequestBuilder`
/// rather than baked into the client.
///
/// Fallible: `reqwest::Client::builder().build()` can fail at runtime (TLS
/// backend init, platform config). Callers propagate the error via `?`
/// rather than panicking; a transient failure is then logged the same as
/// any other HTTP error and the next call retries.
fn shared_client() -> reqwest::Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let new = reqwest::Client::builder()
        .user_agent(default_user_agent())
        .build()?;
    // First-write wins. A concurrent caller may have built its own client
    // already; in that case `set` returns Err and we discard ours.
    let _ = CLIENT.set(new.clone());
    Ok(CLIENT.get().cloned().unwrap_or(new))
}

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

/// Two-step Open Library lookup. Returns `(canonical_url, mime, bytes)` on
/// a hit, `None` on a clean miss, or `Err` on a network / decode error.
async fn fetch_open_library(
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

/// Per-request timeout for a user-supplied "paste image URL" download. More
/// generous than the Open Library timeout because the user is waiting
/// synchronously on an arbitrary remote host.
const REMOTE_IMAGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Errors surfaced by [`fetch_remote_image`]. The variants are deliberately
/// user-facing — the handler maps each to a 4xx/5xx response without further
/// rephrasing.
#[derive(Debug, thiserror::Error)]
pub enum FetchRemoteImageError {
    #[error("URL must start with http:// or https://")]
    BadScheme,
    /// Issue #275 — SSRF guard. The supplied URL parsed cleanly, but either
    /// its host could not be resolved or one of the resolved IPs falls into a
    /// blocked range (loopback / private RFC1918 / link-local / multicast /
    /// IPv6 ULA / IPv6 site-local / unspecified / broadcast / documentation).
    /// We reject *before* any TCP connect so a hostile admin URL can't be
    /// used to probe the loopback / cloud-metadata (`169.254.169.254`) /
    /// internal-network surface area.
    #[error("URL host is not allowed: {0}")]
    BlockedAddress(String),
    #[error("URL is not parseable")]
    InvalidUrl,
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

/// Knobs for [`fetch_remote_image_with`]. Production code constructs
/// [`RemoteImageConfig::default`] (strict: only public IPs are allowed). The
/// `allow_private_addresses` escape hatch exists exclusively for integration
/// tests that need to hit a local `wiremock` server bound to `127.0.0.1` —
/// the production HTTP handlers must never construct a `RemoteImageConfig`
/// with this flag set.
#[derive(Debug, Clone, Default)]
pub struct RemoteImageConfig {
    /// When `true`, [`fetch_remote_image_with`] skips the IP-range check
    /// entirely. Default `false` (derived `Default`). Test-only override —
    /// see the doc comment on this struct.
    pub allow_private_addresses: bool,
}

/// SSRF guard. Returns `true` if `addr` falls into any range we refuse to
/// open a TCP connection against from an admin-supplied URL. Categories:
///   - loopback (127.0.0.0/8, ::1)
///   - unspecified (0.0.0.0, ::)
///   - IPv4 private (RFC 1918: 10/8, 172.16/12, 192.168/16)
///   - IPv4 link-local (169.254/16 — covers AWS/GCP/Azure IMDS at
///     169.254.169.254)
///   - IPv4 multicast (224/4) + broadcast (255.255.255.255)
///   - IPv4 documentation (192.0.2/24, 198.51.100/24, 203.0.113/24)
///   - IPv4 carrier-grade NAT (100.64/10)
///   - IPv4 benchmarking (198.18/15)
///   - IPv6 multicast (ff00::/8)
///   - IPv6 ULA (fc00::/7) — `Ipv6Addr::is_unique_local` is nightly-only
///     so we bit-check the high octet directly.
///   - IPv6 link-local (fe80::/10) — also nightly-only on stable, bit-check.
///   - IPv6 documentation (2001:db8::/32)
///   - IPv6-mapped IPv4 — unwrap to the wrapped IPv4 and re-check, so
///     `::ffff:127.0.0.1` is still loopback.
fn is_blocked_address(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
            {
                return true;
            }
            let oct = v4.octets();
            // 100.64.0.0/10 — RFC 6598 carrier-grade NAT.
            if oct[0] == 100 && (oct[1] & 0xc0) == 64 {
                return true;
            }
            // 198.18.0.0/15 — RFC 2544 benchmarking.
            if oct[0] == 198 && (oct[1] & 0xfe) == 18 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            // IPv6-mapped IPv4 (::ffff:0:0/96) — recurse on the embedded v4
            // so `::ffff:127.0.0.1` is rejected as loopback.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_address(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            // fc00::/7 — Unique Local Addresses (`is_unique_local` is
            // nightly-only on `Ipv6Addr`).
            if (seg[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // fe80::/10 — Link-local (`is_unicast_link_local` is nightly-only).
            if (seg[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // 2001:db8::/32 — documentation.
            if seg[0] == 0x2001 && seg[1] == 0x0db8 {
                return true;
            }
            false
        }
    }
}

/// Parse `url` and resolve its host to a list of pinned `SocketAddr`s with
/// every address gated by [`is_blocked_address`]. Used by
/// [`fetch_remote_image_with`] so the subsequent `reqwest` call is locked to
/// the validated set (defeats DNS rebinding between our check and reqwest's
/// own resolution).
async fn validated_resolve(url: &str) -> Result<(String, Vec<SocketAddr>), FetchRemoteImageError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| FetchRemoteImageError::InvalidUrl)?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(FetchRemoteImageError::BadScheme);
    }
    let host = parsed
        .host_str()
        .ok_or(FetchRemoteImageError::InvalidUrl)?
        .to_string();
    let port = parsed
        .port_or_known_default()
        .ok_or(FetchRemoteImageError::InvalidUrl)?;

    // Fast path: the host is already a literal IP — `lookup_host` would still
    // work, but skipping it avoids spurious DNS lookups on IP-literal URLs.
    if let Ok(literal) = host.parse::<IpAddr>() {
        if is_blocked_address(literal) {
            return Err(FetchRemoteImageError::BlockedAddress(literal.to_string()));
        }
        return Ok((host, vec![SocketAddr::new(literal, port)]));
    }

    let mut addrs: Vec<SocketAddr> = Vec::new();
    let resolved = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|_| FetchRemoteImageError::BlockedAddress(host.clone()))?;
    for sa in resolved {
        if is_blocked_address(sa.ip()) {
            // Reject the *whole* hostname if any A/AAAA record points
            // somewhere private — partial allowlisting would still let a
            // hostile DNS server smuggle a private IP into the resolved set.
            return Err(FetchRemoteImageError::BlockedAddress(sa.ip().to_string()));
        }
        addrs.push(sa);
    }
    if addrs.is_empty() {
        return Err(FetchRemoteImageError::BlockedAddress(host));
    }
    Ok((host, addrs))
}

/// Fetch an image from a user-supplied URL with the same validation gates as
/// the multipart upload route — image content-type, no SVG, size cap. Returns
/// the raw bytes and the server-advertised content-type; callers are
/// expected to run magic-byte sniffing on the bytes before persisting.
///
/// Production callers go through the default strict [`RemoteImageConfig`]
/// SSRF guard: the URL's host is resolved and every resolved IP is checked
/// against [`is_blocked_address`] *before* any TCP connect, then the
/// `reqwest` client is built with `.resolve_to_addrs(host, validated)` so
/// it can't be tricked into re-resolving and hitting a different IP (DNS
/// rebinding).
pub async fn fetch_remote_image(url: &str) -> Result<(String, Vec<u8>), FetchRemoteImageError> {
    fetch_remote_image_with(url, &RemoteImageConfig::default()).await
}

/// [`fetch_remote_image`] with an injectable config. Integration tests that
/// need to hit a `127.0.0.1`-bound `wiremock` server flip
/// `allow_private_addresses` on; everything else (RPC handler, REST
/// handler, follow-up code paths) MUST keep the default.
pub async fn fetch_remote_image_with(
    url: &str,
    config: &RemoteImageConfig,
) -> Result<(String, Vec<u8>), FetchRemoteImageError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(FetchRemoteImageError::BadScheme);
    }

    // Strict mode (the default): resolve host → validate IPs → pin the
    // reqwest client to that set so DNS rebinding can't swap in a private
    // IP between our check and reqwest's own DNS resolution. In test mode
    // (`allow_private_addresses`) we skip the IP-range check but still
    // build a per-call client. Redirects are disabled in BOTH modes: a
    // 3xx pointing at a new host (e.g. `http://169.254.169.254/`) would
    // force reqwest to re-resolve through its default resolver, bypassing
    // the IP-range guard that only applies to the original host. An admin
    // could otherwise host `attacker.com` (passes the guard) and serve a
    // 302 to IMDS to exfiltrate cloud creds. Author-photo URLs are direct
    // image fetches; legitimate sources don't need cross-host redirects.
    let mut builder = reqwest::Client::builder()
        .user_agent(default_user_agent())
        .redirect(reqwest::redirect::Policy::none());
    if !config.allow_private_addresses {
        let (host, addrs) = validated_resolve(url).await?;
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    let client = builder.build()?;

    let resp = client.get(url).timeout(REMOTE_IMAGE_TIMEOUT).send().await?;
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
    // Stream the body and abort the moment we'd cross the cap. A hostile
    // (or buggy) server may advertise no `Content-Length`, or chunk past
    // the cap, or lie about its size — `resp.bytes()` would have
    // happily allocated whatever it sent. Cap memory at
    // `REMOTE_IMAGE_MAX_BYTES + 1` bytes worst case (one chunk overshoot
    // before the check fires).
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp;
    while let Some(chunk) = stream.chunk().await? {
        if buf.len() as u64 + chunk.len() as u64 > REMOTE_IMAGE_MAX_BYTES {
            return Err(FetchRemoteImageError::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((content_type, buf))
}

#[cfg(test)]
mod tests;
