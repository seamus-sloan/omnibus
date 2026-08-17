//! Tests for `EbookMetadata` and `MetadataOverrides`: title/description
//! display fallbacks, override validation (length caps measured in chars,
//! ISBN-13 shape, subject/creator/tag limits), and override-merge layering.
//!
//! Split by sub-topic (mirrors `server/src/backend/opds/tests/`): the
//! `validate()`, `merge()`, and `BulkMetadataEdit` tests live in the
//! sibling modules below; the shared `contributor`/`tags` fixture builders
//! and the small `display_title` tests stay here.

mod bulk;
mod merge;
mod validate;

use super::*;

fn contributor(name: &str) -> Contributor {
    Contributor {
        name: name.to_string(),
        ..Default::default()
    }
}

fn tags(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

// --- display_title() (free fn — label/filename pairs outside EbookMetadata, e.g. book_files) ---

#[test]
fn display_title_helper_returns_title_when_set() {
    assert_eq!(display_title(Some("A Title"), "file.epub"), "A Title");
}

#[test]
fn display_title_helper_falls_back_to_filename_when_title_is_none() {
    assert_eq!(display_title(None, "file.epub"), "file.epub");
}

// --- EbookMetadata::display_title() ------------------------------------

#[test]
fn display_title_returns_title_when_present() {
    let m = EbookMetadata {
        filename: "book.epub".into(),
        title: Some("The Actual Title".into()),
        ..Default::default()
    };
    assert_eq!(m.display_title(), "The Actual Title");
}

#[test]
fn display_title_falls_back_to_filename_when_title_is_none() {
    let m = EbookMetadata {
        filename: "untitled.epub".into(),
        title: None,
        ..Default::default()
    };
    assert_eq!(m.display_title(), "untitled.epub");
}
