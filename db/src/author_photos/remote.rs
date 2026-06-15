//! SSRF-guarded remote image fetch for admin "paste URL" uploads. Resolves
//! the target host, blocks private/loopback/cloud-metadata address ranges
//! before any TCP connect, then pins `reqwest` to the validated addresses so
//! DNS rebinding cannot substitute a blocked IP after the check.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::author_photos::shared::default_user_agent;

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
    /// SSRF guard triggered. The supplied URL parsed cleanly, but either its
    /// host could not be resolved or one of the resolved IPs falls into a
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
pub(super) fn is_blocked_address(addr: IpAddr) -> bool {
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
        .map(std::string::ToString::to_string)
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
