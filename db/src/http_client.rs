//! Shared `reqwest::Client` builder for the crate's outbound HTTP integrations
//! (OpenLibrary, Hardcover, Google Books, author-photo search). Each caller
//! owns its own process-wide `OnceLock<Client>`; this module shares the
//! construction boilerplate only. Not used by `author_photos::remote`, which
//! builds a per-call client pinned to pre-validated addresses for SSRF safety.

/// Build a `reqwest::Client` with the given `User-Agent` header. Fallible
/// (TLS backend init); callers cache the result behind their own
/// `OnceLock` and propagate a build failure via `?`.
pub(crate) fn build_client(user_agent: &str) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().user_agent(user_agent).build()
}

/// This crate's default `User-Agent` header value, shared by every
/// outbound HTTP integration.
pub(crate) fn default_user_agent() -> String {
    format!(
        "omnibus/{} (https://github.com/sloansa/omnibus)",
        env!("CARGO_PKG_VERSION")
    )
}
