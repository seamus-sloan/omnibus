//! Shared process-wide `reqwest::Client` for the Open Library cascade in
//! `author_photos`, built once and cloned so resolutions share one connection
//! pool and TLS session cache. The admin "paste URL" path (`remote.rs`) opts
//! out — it pins `reqwest` to pre-validated addresses for SSRF protection.

use std::sync::OnceLock;

/// Return the crate's default `User-Agent` header value.
pub(super) fn default_user_agent() -> String {
    crate::http_client::default_user_agent()
}

/// Process-wide shared `reqwest::Client`.
///
/// `reqwest::Client` is designed to be cloned and reused: a clone shares the
/// underlying connection pool and TLS session cache. Building one per call
/// (the previous behaviour) paid a fresh TLS handshake on every request and
/// reused no connections across the search + cover-download pair, or across
/// the many author-photo resolutions a deployment fires in sequence.
///
/// We hand out clones of this single client so cascade resolutions share one
/// pool. The per-request timeout differs between call sites, so it's applied
/// on the `RequestBuilder` rather than baked into the client.
///
/// Note: the admin "paste image URL" path (`remote::fetch_remote_image_with`)
/// does **not** use this client — it builds a per-call client pinned to
/// pre-validated socket addresses to prevent SSRF via DNS rebinding.
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
    let new = crate::http_client::build_client(&default_user_agent())?;
    // First-write wins: `get_or_init` returns the existing value if a
    // concurrent caller already initialized `CLIENT`, discarding ours.
    Ok(CLIENT.get_or_init(|| new).clone())
}
