//! Shared HTTP client for outbound requests in `author_photos`. A single
//! process-wide `reqwest::Client` is built once and cloned so all OL cascade
//! resolutions and admin "paste URL" fetches share one connection pool and
//! TLS session cache.

use std::sync::OnceLock;

/// Return the crate's default `User-Agent` header value.
pub(super) fn default_user_agent() -> String {
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
pub(super) fn shared_client() -> reqwest::Result<reqwest::Client> {
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
