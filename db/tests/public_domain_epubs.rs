//! Regression test for the public-domain Playwright EPUB fixtures.
//!
//! These EPUBs come from third-party sources (Project Gutenberg / Standard
//! Ebooks) so we don't control their OPFs. Pinning the metadata the parser
//! extracts from each one means a parser change that subtly drops a field
//! fails here before the Playwright suite gets to it.

use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test_data")
        .join("epubs")
        .join("public_domain")
}

#[test]
fn public_domain_epubs_parse_with_expected_metadata() {
    let dir = fixtures_dir();
    let result = omnibus_db::ebook::scan_ebook_library(Some(dir.to_str().unwrap()));
    assert!(result.error.is_none(), "scan errored: {:?}", result.error);
    assert_eq!(result.books.len(), 4, "expected 4 public-domain epubs");

    let by_name: std::collections::HashMap<&str, _> = result
        .books
        .iter()
        .map(|b| (b.metadata.filename.as_str(), b))
        .collect();

    let dracula = by_name.get("dracula.epub").expect("dracula present");
    assert!(dracula.metadata.error.is_none());
    assert_eq!(dracula.metadata.title.as_deref(), Some("Dracula"));
    assert_eq!(
        dracula
            .metadata
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Bram Stoker"]
    );
    assert_eq!(dracula.metadata.language.as_deref(), Some("en"));
    assert!(dracula.cover.is_some(), "dracula should ship a cover");

    let frankenstein = by_name
        .get("frankenstein.epub")
        .expect("frankenstein present");
    assert!(frankenstein.metadata.error.is_none());
    assert_eq!(
        frankenstein.metadata.title.as_deref(),
        Some("Frankenstein; or, the modern prometheus")
    );
    assert_eq!(
        frankenstein
            .metadata
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Mary Wollstonecraft Shelley"]
    );
    assert!(frankenstein.cover.is_some());

    let gatsby = by_name
        .get("great_gatsby.epub")
        .expect("great_gatsby present");
    assert!(gatsby.metadata.error.is_none());
    assert_eq!(gatsby.metadata.title.as_deref(), Some("The Great Gatsby"));
    assert_eq!(
        gatsby
            .metadata
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["F. Scott Fitzgerald"]
    );
    assert!(gatsby.cover.is_some());

    let romeo = by_name
        .get("romeo_and_juliet.epub")
        .expect("romeo_and_juliet present");
    assert!(romeo.metadata.error.is_none());
    assert_eq!(romeo.metadata.title.as_deref(), Some("Romeo and Juliet"));
    assert_eq!(
        romeo
            .metadata
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["William Shakespeare"]
    );
    assert!(romeo.cover.is_some());
}
