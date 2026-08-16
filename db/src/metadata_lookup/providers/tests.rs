//! Tests for [`super::catalog`]: AC1's per-provider `configured` semantics,
//! and AC3's cross-check against [`super::ladder`] — the two must never
//! disagree about what "configured" means, across every key combination.

use omnibus_shared::metadata_lookup::MetadataProvider;

use super::*;
use crate::metadata_lookup::ProviderKeys;
use crate::test_support::EnvVarGuard;

/// Build a config with explicit keys, no env involved — used for the
/// AC1-focused tests, which only care about `ProviderKeys` values, not the
/// environment.
fn config_with(googlebooks: Option<&str>, hardcover: Option<&str>) -> MetadataLookupConfig {
    MetadataLookupConfig::live(ProviderKeys {
        googlebooks: googlebooks.map(str::to_string),
        hardcover: hardcover.map(str::to_string),
    })
}

/// Every provider `ladder` would invoke for `config` must be `configured` in
/// `catalog(config)` — the AC3 cross-check, shared by every combo below.
fn assert_catalog_agrees_with_ladder(config: &MetadataLookupConfig) {
    let entries = catalog(config);
    for rung in ladder(config) {
        let entry = entries
            .iter()
            .find(|e| e.id == rung.provider)
            .unwrap_or_else(|| panic!("catalog is missing a ladder provider: {:?}", rung.provider));
        assert!(
            entry.configured,
            "ladder includes {:?} but catalog reports it unconfigured",
            rung.provider
        );
    }
}

#[test]
fn catalog_reports_open_library_configured_with_no_keys() {
    let config = config_with(None, None);
    let entry = catalog(&config)
        .into_iter()
        .find(|e| e.id == MetadataProvider::OpenLibrary)
        .expect("catalog should list Open Library");
    assert!(entry.configured);
    assert!(!entry.requires_key);
    assert_eq!(entry.display_name, "Open Library");
}

#[test]
fn catalog_reports_hardcover_unconfigured_without_a_key() {
    let config = config_with(None, None);
    let entry = catalog(&config)
        .into_iter()
        .find(|e| e.id == MetadataProvider::Hardcover)
        .expect("catalog should list Hardcover");
    assert!(!entry.configured);
    assert!(entry.requires_key);
}

#[test]
fn catalog_reports_hardcover_configured_with_a_key() {
    let config = config_with(None, Some("hc_test_key"));
    let entry = catalog(&config)
        .into_iter()
        .find(|e| e.id == MetadataProvider::Hardcover)
        .expect("catalog should list Hardcover");
    assert!(entry.configured);
}

#[test]
fn catalog_reports_google_books_configured_with_no_key() {
    let config = config_with(None, None);
    let entry = catalog(&config)
        .into_iter()
        .find(|e| e.id == MetadataProvider::GoogleBooks)
        .expect("catalog should list Google Books");
    assert!(
        entry.configured,
        "Google Books is tried keyless too, just not as the ladder's primary rung"
    );
    assert!(!entry.requires_key);
}

#[test]
fn catalog_reports_google_books_configured_with_a_key() {
    let config = config_with(Some("gb_test_key"), None);
    let entry = catalog(&config)
        .into_iter()
        .find(|e| e.id == MetadataProvider::GoogleBooks)
        .expect("catalog should list Google Books");
    assert!(entry.configured);
}

#[test]
fn catalog_never_carries_key_material() {
    let config = config_with(Some("gb_secret_value"), Some("hc_secret_value"));
    let json = serde_json::to_string(&catalog(&config)).expect("catalog should serialize");
    assert!(!json.contains("gb_secret_value"));
    assert!(!json.contains("hc_secret_value"));
}

// AC3: `catalog` and `ladder` never disagree about what "configured" means,
// across all four (Hardcover key set/unset) x (Google Books key set/unset)
// combinations. `EnvVarGuard` pins both env vars so a developer's real
// `.env` can't change which branch `ProviderKeys::from_env()` takes.

#[test]
fn catalog_and_ladder_agree_when_no_keys_are_configured() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None).also_set("GOOGLE_BOOKS_API_KEY", None);
    let config = MetadataLookupConfig::live(ProviderKeys::from_env());
    assert_catalog_agrees_with_ladder(&config);
}

#[test]
fn catalog_and_ladder_agree_when_only_google_books_key_is_configured() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None)
        .also_set("GOOGLE_BOOKS_API_KEY", Some("gb_test_key"));
    let config = MetadataLookupConfig::live(ProviderKeys::from_env());
    assert_catalog_agrees_with_ladder(&config);
}

#[test]
fn catalog_and_ladder_agree_when_only_hardcover_key_is_configured() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("hc_test_key"))
        .also_set("GOOGLE_BOOKS_API_KEY", None);
    let config = MetadataLookupConfig::live(ProviderKeys::from_env());
    assert_catalog_agrees_with_ladder(&config);
}

#[test]
fn catalog_and_ladder_agree_when_both_keys_are_configured() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("hc_test_key"))
        .also_set("GOOGLE_BOOKS_API_KEY", Some("gb_test_key"));
    let config = MetadataLookupConfig::live(ProviderKeys::from_env());
    assert_catalog_agrees_with_ladder(&config);
}
