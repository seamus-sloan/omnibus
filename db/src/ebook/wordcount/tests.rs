//! Unit tests for [`super::estimate_word_count`] and its `strip_tags` helper.

use epub::doc::EpubDoc;

use super::*;
use crate::ebook::test_support::fixture;
use crate::test_support::build_stored_zip;

#[test]
fn strip_tags_drops_markup_and_keeps_text() {
    // No whitespace separates `</h1>` and `<p>` in the source, so the
    // stripped text runs together at that boundary too — `strip_tags` never
    // inserts a space of its own, `split_whitespace` (the real caller) is
    // what's forgiving of that.
    let html = "<html><body><h1>Alpha</h1><p>Synthetic test content.</p></body></html>";
    assert_eq!(strip_tags(html), "AlphaSynthetic test content.");
}

#[test]
fn strip_tags_handles_attributes_and_self_closing_tags() {
    let html = r#"<p class="x">One<br/>two</p>"#;
    assert_eq!(strip_tags(html), "Onetwo");
}

#[test]
fn estimate_word_count_sums_words_across_the_spine() {
    // alpha.epub's sole spine item is `<h1>Alpha</h1><p>Synthetic test
    // content.</p>` — 4 whitespace-separated tokens once markup is stripped.
    let mut doc = EpubDoc::new(fixture("alpha.epub")).expect("open fixture epub");
    assert_eq!(estimate_word_count(&mut doc), Some(4));
}

#[test]
fn estimate_word_count_returns_none_when_spine_is_empty() {
    let zip = minimal_zip_with_empty_spine();
    let mut doc = EpubDoc::from_reader(std::io::Cursor::new(zip)).expect("parse in-memory epub");
    assert_eq!(estimate_word_count(&mut doc), None);
}

/// Smallest valid (uncompressed) EPUB container whose spine has zero
/// itemrefs — enough for `EpubDoc` to parse, with nothing for
/// `estimate_word_count` to sum.
fn minimal_zip_with_empty_spine() -> Vec<u8> {
    let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Empty</dc:title></metadata>
  <manifest></manifest>
  <spine></spine>
</package>"#;
    let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    build_stored_zip(&[
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", container),
        ("content.opf", opf),
    ])
}
