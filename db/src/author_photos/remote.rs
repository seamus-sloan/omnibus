//! The SSRF-guarded remote image fetch — the **only** one in this workspace.
//! Resolves the target host, blocks private/loopback/cloud-metadata address
//! ranges before any TCP connect, then pins `reqwest` to the validated
//! addresses so DNS rebinding cannot substitute a blocked IP after the check.
//!
//! Two callers with different terms, one implementation: the admin "paste
//! URL" photo upload takes it as-is, and the provider-cover apply tightens it
//! through [`RemoteImageConfig`] (host allowlist, HTTPS-only, a bounded
//! redirect follow). A second fetcher for the second caller is the thing this
//! module exists to prevent.

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
    /// A pre-persist validation gate failed (scheme, URL, status, content-type,
    /// SVG, or size cap); the message names which, and the handler maps it to 400.
    #[error("{0}")]
    Validation(String),
    /// SSRF guard triggered. The supplied URL parsed cleanly, but either its
    /// host could not be resolved or one of the resolved IPs falls into a
    /// blocked range (loopback / private RFC1918 / link-local / multicast /
    /// IPv6 ULA / IPv6 site-local / unspecified / broadcast / documentation).
    /// We reject *before* any TCP connect so a hostile admin URL can't be
    /// used to probe the loopback / cloud-metadata (`169.254.169.254`) /
    /// internal-network surface area.
    #[error("URL host is not allowed: {0}")]
    BlockedAddress(String),
    /// The host is publicly routable but not on the caller's allowlist. A
    /// distinct variant from [`Self::BlockedAddress`] because the two are
    /// different refusals: one is "that address is inside our network", the
    /// other is "we only fetch covers from the sources we know".
    #[error("host is not an allowed source: {0}")]
    HostNotAllowed(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// Validation error for a URL that doesn't parse or lacks a usable host/port.
fn invalid_url() -> FetchRemoteImageError {
    FetchRemoteImageError::Validation("URL is not parseable".into())
}

/// Validation error for a URL whose scheme isn't `http`/`https`.
fn bad_scheme() -> FetchRemoteImageError {
    FetchRemoteImageError::Validation("URL must start with http:// or https://".into())
}

/// Validation error for a download that exceeds [`REMOTE_IMAGE_MAX_BYTES`].
fn too_large() -> FetchRemoteImageError {
    FetchRemoteImageError::Validation(format!("image exceeds {REMOTE_IMAGE_MAX_BYTES} byte cap"))
}

/// Knobs for [`fetch_remote_image_with`]. Production code constructs
/// [`RemoteImageConfig::default`] (strict: only public IPs are allowed) or
/// derives from it. The `allow_private_addresses` escape hatch exists
/// exclusively for integration tests that need to hit a local `wiremock`
/// server bound to `127.0.0.1` — the production HTTP handlers must never
/// construct a `RemoteImageConfig` with this flag set.
///
/// The remaining three fields tighten the default rather than relaxing it,
/// and exist so the provider-cover fetch can share this one SSRF
/// implementation instead of growing a second: `Default` reproduces the
/// admin paste-a-URL behaviour exactly (any host, `http` allowed, redirects
/// refused), and the cover path opts into a host allowlist, HTTPS-only, and
/// a bounded follow.
#[derive(Debug, Clone, Default)]
pub struct RemoteImageConfig {
    /// When `true`, [`fetch_remote_image_with`] skips the IP-range check
    /// entirely. Default `false` (derived `Default`). Test-only override —
    /// see the doc comment on this struct.
    pub allow_private_addresses: bool,
    /// When non-empty, the URL's host — **and every redirect hop's host** —
    /// must match one of these entries. An entry beginning `*.` matches any
    /// subdomain of the rest; everything else is an exact, case-insensitive
    /// match. Empty (the default) means no host restriction.
    pub host_allowlist: Vec<String>,
    /// When `true`, only `https` is accepted, at the original URL and at
    /// every redirect hop. Default `false`, which still accepts `http`.
    pub require_https: bool,
    /// How many redirects to follow. Default `0` — refuse them outright,
    /// which is what the author-photo path wants. Following is only safe
    /// because every hop is re-validated against the same gates as the
    /// original URL; never raise this without keeping that true.
    pub max_redirects: u8,
}

impl RemoteImageConfig {
    /// Whether `host` is allowed under this config. An empty allowlist
    /// allows everything, matching the pre-allowlist behaviour.
    ///
    /// The wildcard deliberately does **not** match the bare parent domain:
    /// `*.archive.org` matches `ia1.us.archive.org` but not `archive.org`,
    /// which must be listed on its own. A single-label suffix (`*.com`) is
    /// refused outright rather than treated as a wildcard over a whole TLD.
    pub(super) fn host_allowed(&self, host: &str) -> bool {
        if self.host_allowlist.is_empty() {
            return true;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.host_allowlist.iter().any(|entry| {
            let entry = entry.to_ascii_lowercase();
            match entry.strip_prefix("*.") {
                Some(suffix) => {
                    suffix.contains('.')
                        && host.len() > suffix.len() + 1
                        && host.ends_with(suffix)
                        && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
                }
                None => host == entry,
            }
        })
    }
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
async fn validated_resolve(
    url: &reqwest::Url,
    config: &RemoteImageConfig,
) -> Result<(String, Vec<SocketAddr>), FetchRemoteImageError> {
    let parsed = url;
    let host = parsed.host_str().ok_or_else(invalid_url)?.to_string();
    let port = parsed.port_or_known_default().ok_or_else(invalid_url)?;

    // Fast path: the host is already a literal IP — `lookup_host` would still
    // work, but skipping it avoids spurious DNS lookups on IP-literal URLs.
    if let Ok(literal) = host.parse::<IpAddr>() {
        if !config.allow_private_addresses && is_blocked_address(literal) {
            return Err(FetchRemoteImageError::BlockedAddress(literal.to_string()));
        }
        return Ok((host, vec![SocketAddr::new(literal, port)]));
    }

    let mut addrs: Vec<SocketAddr> = Vec::new();
    let resolved = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|_| FetchRemoteImageError::BlockedAddress(host.clone()))?;
    for sa in resolved {
        if !config.allow_private_addresses && is_blocked_address(sa.ip()) {
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
/// handler, follow-up code paths) MUST keep it off.
///
/// The gates run in this order, and the order matters: scheme, then host
/// allowlist, then IP range — **for the original URL and independently for
/// every redirect hop**. Redirects are only followed at all when
/// [`RemoteImageConfig::max_redirects`] says so, and a hop that fails any
/// gate ends the fetch rather than being followed and checked afterwards.
pub async fn fetch_remote_image_with(
    url: &str,
    config: &RemoteImageConfig,
) -> Result<(String, Vec<u8>), FetchRemoteImageError> {
    let mut current = parse_and_gate(url, config)?;

    // Follow at most `max_redirects` hops, re-running every gate on each one.
    // `Policy::none()` on the client is what makes that possible: reqwest
    // hands back the 3xx instead of chasing it through its own resolver,
    // which would bypass the IP-range guard entirely.
    for _ in 0..=config.max_redirects {
        let resp = send_one(&current, config).await?;
        let status = resp.status();
        if status.is_redirection() {
            if config.max_redirects == 0 {
                return Err(FetchRemoteImageError::Validation(format!(
                    "remote server returned {}",
                    status.as_u16()
                )));
            }
            current = next_hop(&current, &resp, config)?;
            continue;
        }
        if !status.is_success() {
            return Err(FetchRemoteImageError::Validation(format!(
                "remote server returned {}",
                status.as_u16()
            )));
        }
        return read_image_body(resp).await;
    }
    Err(FetchRemoteImageError::Validation(format!(
        "too many redirects (limit {})",
        config.max_redirects
    )))
}

/// Parse a URL and run the pre-connect gates: scheme, then host allowlist.
///
/// Applied to the original URL **and** to every redirect target, which is the
/// whole point — trusting the first hop and following the rest is how an
/// allowlisted host becomes an open redirect into the server's own network.
fn parse_and_gate(
    url: &str,
    config: &RemoteImageConfig,
) -> Result<reqwest::Url, FetchRemoteImageError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| invalid_url())?;
    match parsed.scheme() {
        "https" => {}
        "http" if !config.require_https => {}
        "http" => {
            return Err(FetchRemoteImageError::Validation(
                "URL must start with https://".into(),
            ))
        }
        _ => return Err(bad_scheme()),
    }
    let host = parsed.host_str().ok_or_else(invalid_url)?;
    if !config.host_allowed(host) {
        return Err(FetchRemoteImageError::HostNotAllowed(host.to_string()));
    }
    Ok(parsed)
}

/// Resolve, pin, and GET one hop.
async fn send_one(
    url: &reqwest::Url,
    config: &RemoteImageConfig,
) -> Result<reqwest::Response, FetchRemoteImageError> {
    // Strict mode (the default): resolve host → validate IPs → pin the
    // reqwest client to that set so DNS rebinding can't swap in a private
    // IP between our check and reqwest's own DNS resolution. In test mode
    // (`allow_private_addresses`) we skip the IP-range check but still
    // build a per-call client.
    let mut builder = reqwest::Client::builder()
        .user_agent(default_user_agent())
        .redirect(reqwest::redirect::Policy::none());
    if !config.allow_private_addresses {
        let (host, addrs) = validated_resolve(url, config).await?;
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    Ok(builder
        .build()?
        .get(url.clone())
        .timeout(REMOTE_IMAGE_TIMEOUT)
        .send()
        .await?)
}

/// The next URL to fetch, from a 3xx response — re-parsed, re-gated, and
/// resolved relative to the hop it came from.
///
/// `pub(super)` so the sibling test module can drive it directly: the per-hop
/// scheme recheck cannot be reached end-to-end from a `wiremock` origin,
/// which is plaintext, so hop 1 would be refused before hop 2 existed.
pub(super) fn next_hop(
    from: &reqwest::Url,
    resp: &reqwest::Response,
    config: &RemoteImageConfig,
) -> Result<reqwest::Url, FetchRemoteImageError> {
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            FetchRemoteImageError::Validation("redirect carried no usable Location".into())
        })?;
    // Relative Locations are legal and common, so join rather than parse —
    // then gate the *result*, which is the URL actually about to be fetched.
    let joined = from.join(location).map_err(|_| invalid_url())?;
    parse_and_gate(joined.as_str(), config)
}

/// Read a success response as image bytes, applying the content-type, SVG,
/// and size gates.
async fn read_image_body(
    resp: reqwest::Response,
) -> Result<(String, Vec<u8>), FetchRemoteImageError> {
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
        .unwrap_or_else(|| "application/octet-stream".into());
    if !content_type.starts_with("image/") {
        return Err(FetchRemoteImageError::Validation(format!(
            "remote response content-type is not an image ({content_type})"
        )));
    }
    if content_type.contains("svg") {
        return Err(FetchRemoteImageError::Validation(
            "SVG photos are not accepted".into(),
        ));
    }
    // Pre-check Content-Length when the server advertises it so an
    // obviously-oversized download bails before allocating.
    if let Some(len) = resp.content_length() {
        if len > REMOTE_IMAGE_MAX_BYTES {
            return Err(too_large());
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
            return Err(too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((content_type, buf))
}
