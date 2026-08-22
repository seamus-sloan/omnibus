//! The terms a provider cover is fetched under.
//!
//! Applying a source's cover means the server fetching a client-supplied URL,
//! which is a textbook SSRF gadget aimed at the server's own network. The
//! allowlist is the control, not a nicety — and it is [`super::cover_hosts`],
//! the same list the `img-src` CSP is built from, so a host the picker can
//! render is exactly a host a cover can be applied from.

use crate::author_photos::RemoteImageConfig;

/// Redirect hops allowed when fetching a provider cover.
///
/// Not zero, because Open Library's cover CDN genuinely needs two: it 302s to
/// `archive.org`, which 302s again to the node holding the file. Four leaves
/// headroom without turning the fetch into a crawl. Every hop is re-checked
/// against the same scheme and allowlist gates as the original URL — the
/// follow is only safe because of that.
pub const MAX_COVER_REDIRECTS: u8 = 4;

/// The fetch config for a provider cover: HTTPS only, hosts limited to the
/// provider catalog's, and a bounded redirect follow.
///
/// `allow_private_addresses` is threaded through rather than defaulted so a
/// test can point the fetch at a loopback `wiremock` origin; production
/// callers pass `false` and every other gate still applies either way.
pub fn provider_cover_image_config(allow_private_addresses: bool) -> RemoteImageConfig {
    RemoteImageConfig {
        allow_private_addresses,
        host_allowlist: super::all_cover_hosts()
            .into_iter()
            .map(str::to_string)
            .collect(),
        require_https: true,
        max_redirects: MAX_COVER_REDIRECTS,
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::metadata_lookup::MetadataProvider;

    use super::*;
    use crate::metadata_lookup::cover_hosts;

    #[test]
    fn provider_cover_config_allows_every_host_the_catalog_publishes() {
        let config = provider_cover_image_config(false);
        for provider in MetadataProvider::ALL.iter().copied() {
            for host in cover_hosts(provider) {
                assert!(
                    config.host_allowlist.iter().any(|h| h == host),
                    "{host} renders in the picker but could not be applied"
                );
            }
        }
    }

    #[test]
    fn provider_cover_config_is_https_only_and_follows_a_bounded_number_of_hops() {
        let config = provider_cover_image_config(false);
        assert!(config.require_https);
        assert!(!config.allow_private_addresses);
        // Two is the real requirement (Open Library's double redirect); the
        // assertion is that it is bounded, not that it is generous.
        assert!(config.max_redirects >= 2);
        assert!(config.max_redirects <= 8);
    }
}
