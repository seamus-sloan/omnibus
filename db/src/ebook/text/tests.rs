//! Unit tests for [`super::extract_chapter_text`]: happy path against a
//! built EPUB and the committed generated fixture, the out-of-range `None`,
//! the unreadable-file error, and the block-break/inline-tag text shape.

use std::path::PathBuf;

use crate::test_support::{build_test_epub, make_test_dir};

use super::*;

fn write_epub(name: &str, spine: &[(&str, &str)]) -> PathBuf {
    let dir = make_test_dir("chapter_text");
    let path = dir.join(name);
    std::fs::write(&path, build_test_epub(spine)).unwrap();
    path
}

#[test]
fn extract_chapter_text_returns_prose_for_a_spine_index() {
    let path = write_epub(
        "two.epub",
        &[
            (
                "c1.xhtml",
                "<html><body><p>First chapter.</p></body></html>",
            ),
            (
                "c2.xhtml",
                "<html><body><p>Second chapter.</p></body></html>",
            ),
        ],
    );
    assert_eq!(
        extract_chapter_text(&path, 1).unwrap(),
        Some("Second chapter.".to_string())
    );
}

#[test]
fn extract_chapter_text_separates_blocks_and_keeps_inline_tags_whole() {
    let path = write_epub(
        "blocks.epub",
        &[(
            "c1.xhtml",
            "<html><head><title>Head Title</title></head>\
             <body><h1>One</h1><p>cu<i>rio</i>us line<br/>break</p><p>Two</p></body></html>",
        )],
    );
    // Block tags become paragraph/line breaks, the inline <i> never splits
    // its word, and the <title> head text is suppressed entirely.
    assert_eq!(
        extract_chapter_text(&path, 0).unwrap(),
        Some("One\n\ncurious line\nbreak\n\nTwo".to_string())
    );
}

#[test]
fn extract_chapter_text_returns_none_when_spine_index_is_out_of_range() {
    let path = write_epub(
        "one.epub",
        &[("c1.xhtml", "<html><body><p>Only.</p></body></html>")],
    );
    assert_eq!(extract_chapter_text(&path, 5).unwrap(), None);
}

#[test]
fn extract_chapter_text_errors_when_the_file_is_not_an_epub() {
    let dir = make_test_dir("chapter_text_bad");
    let path = dir.join("bad.epub");
    std::fs::write(&path, b"not a zip archive").unwrap();
    assert!(extract_chapter_text(&path, 0).is_err());
}

#[test]
fn extract_chapter_text_reads_the_committed_generated_fixture() {
    // `alpha.epub` mirrors the Playwright fixture table: one spine document
    // holding an <h1> and a <p> (see `db/tests/fixture_epubs.rs`).
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test_data/epubs/generated/alpha.epub");
    assert_eq!(
        extract_chapter_text(&path, 0).unwrap(),
        Some("Alpha\n\nSynthetic test content.".to_string())
    );
    assert_eq!(extract_chapter_text(&path, 99).unwrap(), None);
}
