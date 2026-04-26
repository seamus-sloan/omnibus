//! Regression test for the synthetic Playwright EPUB fixtures.
//!
//! Catches drift between `ui_tests/playwright/tools/make_epub.ts` and the OPF
//! parser in `omnibus_db::ebook` without needing to spin up Playwright. If
//! you regenerate the fixtures and the parser disagrees with what the
//! generator wrote, this test fails first — well before the Playwright
//! suite gets to it.

use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at `db/`; fixtures live at `<repo>/test_data/epubs/generated`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test_data")
        .join("epubs")
        .join("generated")
}

#[test]
fn fixture_epubs_parse_with_expected_metadata() {
    let dir = fixtures_dir();
    let result = omnibus_db::ebook::scan_ebook_library(Some(dir.to_str().unwrap()));
    assert!(result.error.is_none(), "scan errored: {:?}", result.error);
    assert_eq!(result.books.len(), 3, "expected 3 fixture epubs");

    let by_name: std::collections::HashMap<&str, _> = result
        .books
        .iter()
        .map(|b| (b.metadata.filename.as_str(), b))
        .collect();

    let alpha = by_name.get("alpha.epub").expect("alpha fixture present");
    assert!(alpha.metadata.error.is_none());
    assert_eq!(alpha.metadata.title.as_deref(), Some("Alpha"));
    assert_eq!(
        alpha
            .metadata
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Ada Lovelace"]
    );
    assert_eq!(
        alpha.metadata.publisher.as_deref(),
        Some("Omnibus Test Press")
    );
    assert_eq!(alpha.metadata.published.as_deref(), Some("1843-10-01"));
    assert_eq!(alpha.metadata.language.as_deref(), Some("en"));
    assert!(alpha.cover.is_some(), "alpha should ship a cover");

    let beta = by_name.get("beta.epub").expect("beta fixture present");
    assert!(beta.metadata.error.is_none());
    assert_eq!(beta.metadata.title.as_deref(), Some("Beta in the Series"));
    assert_eq!(
        beta.metadata
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Grace Hopper", "Margaret Hamilton"]
    );
    assert_eq!(beta.metadata.series.as_deref(), Some("Pioneers"));
    assert_eq!(beta.metadata.series_index.as_deref(), Some("1"));
    assert!(beta.cover.is_some(), "beta should ship a cover");

    let gamma = by_name.get("gamma.epub").expect("gamma fixture present");
    assert!(gamma.metadata.error.is_none());
    assert_eq!(gamma.metadata.title.as_deref(), Some("Gamma sin Cover"));
    assert_eq!(
        gamma.metadata.publisher.as_deref(),
        Some("Editorial Omnibus")
    );
    assert_eq!(gamma.metadata.language.as_deref(), Some("es"));
    assert_eq!(gamma.metadata.series.as_deref(), Some("Pioneers"));
    assert_eq!(gamma.metadata.series_index.as_deref(), Some("2"));
    assert!(gamma.cover.is_none(), "gamma should not ship a cover");
}
