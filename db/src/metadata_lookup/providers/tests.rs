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

/// A real `imageLinks.thumbnail` value, in the exact shape Google returns it
/// (bar the scheme, which `upgrade_to_https` has already fixed by this point).
const GB_THUMBNAIL: &str = "https://books.google.com/books/content?id=B1hSG45JCX4C&printsec=frontcover&img=1&zoom=1&edge=curl&source=gbs_api";

#[test]
fn upgrade_cover_url_raises_the_zoom_and_drops_the_page_curl() {
    let upgraded = googlebooks::upgrade_cover_url(GB_THUMBNAIL).expect("google books url");
    assert!(upgraded.contains("zoom=0"), "{upgraded}");
    assert!(!upgraded.contains("zoom=1"), "{upgraded}");
    assert!(!upgraded.contains("edge=curl"), "{upgraded}");
}

#[test]
fn upgrade_cover_url_preserves_the_volume_id_and_every_other_parameter() {
    let upgraded = googlebooks::upgrade_cover_url(GB_THUMBNAIL).expect("google books url");
    // The id is what names the book; losing it would silently fetch nothing.
    assert!(upgraded.contains("id=B1hSG45JCX4C"), "{upgraded}");
    assert!(upgraded.contains("printsec=frontcover"), "{upgraded}");
    assert!(upgraded.contains("img=1"), "{upgraded}");
    assert!(upgraded.contains("source=gbs_api"), "{upgraded}");
}

#[test]
fn upgrade_cover_url_stays_on_the_allowlisted_host_and_path() {
    let upgraded = googlebooks::upgrade_cover_url(GB_THUMBNAIL).expect("google books url");
    // The rewrite must not be able to move the fetch off the cover allowlist.
    assert!(
        upgraded.starts_with("https://books.google.com/books/content?"),
        "{upgraded}"
    );
}

#[test]
fn upgrade_cover_url_adds_a_zoom_when_the_url_carries_none() {
    let upgraded = googlebooks::upgrade_cover_url(
        "https://books.google.com/books/content?id=abc&printsec=frontcover",
    )
    .expect("google books url");
    assert!(upgraded.contains("zoom=0"), "{upgraded}");
}

#[test]
fn upgrade_cover_url_declines_a_url_from_another_provider() {
    // Open Library and Hardcover covers are already served at full size; a
    // zoom parameter would be meaningless on them.
    assert_eq!(
        googlebooks::upgrade_cover_url("https://covers.openlibrary.org/b/id/1-L.jpg"),
        None
    );
    assert_eq!(
        googlebooks::upgrade_cover_url("https://assets.hardcover.app/x.jpg"),
        None
    );
}

#[test]
fn upgrade_cover_url_declines_a_google_url_that_is_not_a_cover() {
    // Same host, different path — a search or volume URL is not a bitmap, and
    // rewriting its query would produce a request for something else entirely.
    assert_eq!(
        googlebooks::upgrade_cover_url("https://books.google.com/books?id=abc"),
        None
    );
}

#[test]
fn upgrade_cover_url_declines_a_lookalike_host() {
    // The rewrite is what decides a second fetch happens, so a host that
    // merely *contains* the real one must not match it.
    assert_eq!(
        googlebooks::upgrade_cover_url("https://books.google.com.evil.test/books/content?id=a"),
        None
    );
    assert_eq!(googlebooks::upgrade_cover_url("not a url at all"), None);
}
