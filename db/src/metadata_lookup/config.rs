//! Connection config for the metadata providers: the endpoint each one is
//! reached at, and the API keys that decide which are reachable at all. Base
//! URLs are injectable so tests point them at a local `wiremock` server.

use std::time::Duration;

/// Per-request timeout for a provider call. One ladder walk runs per scan, so
/// this bounds how long the check-in flow waits on a slow provider — and it is
/// applied to **every** provider, including Hardcover, whose own suggestions
/// client allows 30s. A ladder rung that hung that long would leave the reader
/// staring at a spinner while the remaining rungs waited their turn.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

/// Environment variable holding the Google Books API key.
const GOOGLE_BOOKS_API_KEY_ENV: &str = "GOOGLE_BOOKS_API_KEY";

/// Environment variable holding the Hardcover API key.
const HARDCOVER_API_KEY_ENV: &str = "HARDCOVER_API_KEY";

/// The provider API keys, already resolved settings-over-env by the caller
/// (`crate::provider_keys`). A `None` key means that provider is not
/// configured, and the ladder skips it rather than issuing a request that can
/// only fail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderKeys {
    /// Optional: without one the API still answers, but on a shared anonymous
    /// daily quota that a self-hosted instance will hit (HTTP 429) — in
    /// practice keyless lookups fail more often than they succeed. Google
    /// Books is therefore tried either way, just not as the primary.
    pub googlebooks: Option<String>,
    /// Required to reach Hardcover at all — it authenticates every request.
    pub hardcover: Option<String>,
}

impl ProviderKeys {
    /// Keys taken from the environment. The fallback for contexts without a DB
    /// pool (tests, `Default`); a request path with a pool should prefer
    /// `crate::provider_keys`, since the saved Settings value wins over the
    /// env var.
    pub fn from_env() -> Self {
        Self {
            googlebooks: env_key(GOOGLE_BOOKS_API_KEY_ENV),
            hardcover: env_key(HARDCOVER_API_KEY_ENV),
        }
    }
}

fn env_key(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|k| !k.trim().is_empty())
}

/// Where each provider lives and how it authenticates.
#[derive(Debug, Clone)]
pub struct MetadataLookupConfig {
    /// `https://openlibrary.org` in production.
    pub openlibrary_base: String,
    /// `https://www.googleapis.com` in production.
    pub googlebooks_base: String,
    /// `https://api.hardcover.app/v1/graphql` in production.
    pub hardcover_base: String,
    /// Never logged; see `providers::http::strip_url`.
    pub keys: ProviderKeys,
    /// Which providers are currently in a rate-limit cooldown.
    ///
    /// Carried on the config rather than on the server's `AppState` because
    /// the two halves of this feature build their configs independently — the
    /// REST handler from `AppState`, the web server function from a bare pool
    /// — and neither can see the other's router state. A tracker hung off the
    /// router would quietly exempt every web request from throttling.
    pub throttle: super::throttle::ThrottleTracker,
    /// Per-request timeout applied to each provider HTTP call.
    pub timeout: Duration,
}

impl MetadataLookupConfig {
    /// Config pointing at the live provider endpoints with already-resolved
    /// keys. Callers pass the effective keys from settings so a saved value
    /// wins over the env var without this crate needing a pool.
    pub fn live(keys: ProviderKeys) -> Self {
        Self {
            openlibrary_base: "https://openlibrary.org".to_string(),
            googlebooks_base: "https://www.googleapis.com".to_string(),
            hardcover_base: "https://api.hardcover.app/v1/graphql".to_string(),
            keys,
            // The process-wide tracker: a cooldown learned while serving one
            // request has to be known to the next, which is the entire point.
            throttle: super::throttle::ThrottleTracker::shared(),
            timeout: LOOKUP_TIMEOUT,
        }
    }
}

impl Default for MetadataLookupConfig {
    fn default() -> Self {
        Self::live(ProviderKeys::from_env())
    }
}
