//! Shared `reqwest::Client` builder for the crate's outbound HTTP
//! integrations (OpenLibrary, Hardcover, Google Books, author-photo search).
//! Each caller owns its own process-wide `OnceLock<Client>` instance — this
//! module only shares the construction boilerplate, not the client itself.
//!
//! Not used by the admin "paste image URL" path (`author_photos::remote`),
//! which builds a per-call client pinned to pre-validated socket addresses
//! for SSRF protection.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_client_succeeds_with_a_default_user_agent() {
        let client = build_client(&default_user_agent());
        assert!(client.is_ok());
    }

    #[test]
    fn default_user_agent_names_the_crate_and_repo() {
        let ua = default_user_agent();
        assert!(ua.starts_with("omnibus/"));
        assert!(ua.contains("github.com/sloansa/omnibus"));
    }
}
