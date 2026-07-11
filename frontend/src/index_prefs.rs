//! Client-local persistence for the discovery-index sort choices
//! ([`DiscoveryPrefs`]), layered on [`crate::client_store`] (localStorage on
//! web, a JSON file on mobile, inert on SSR). A single library-wide key — the
//! Authors and Series indexes aren't scoped per library path.

use omnibus_shared::DiscoveryPrefs;

const STORAGE_KEY: &str = "omnibus.discovery_prefs";

/// Load the persisted discovery sort choices, or [`DiscoveryPrefs::default`]
/// when nothing is stored or the JSON fails to parse.
pub fn load() -> DiscoveryPrefs {
    crate::client_store::get(STORAGE_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist `prefs`. Serialization/storage failures are silently ignored — the
/// UI keeps its in-memory copy regardless.
pub fn save(prefs: &DiscoveryPrefs) {
    if let Ok(raw) = serde_json::to_string(prefs) {
        crate::client_store::set(STORAGE_KEY, &raw);
    }
}

#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    #[test]
    fn load_returns_defaults_under_ssr() {
        // SSR `client_store` is inert, so load always yields defaults and save
        // can't change that — the contract the discovery pages rely on for
        // hydration parity.
        save(&DiscoveryPrefs {
            authors_sort: omnibus_shared::IndexSort::BookCount,
            ..Default::default()
        });
        assert_eq!(load(), DiscoveryPrefs::default());
    }
}
