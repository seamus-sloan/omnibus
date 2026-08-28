//! Unit tests for [`super::estimate_word_count`] and its `strip_tags` helper.

use epub::doc::EpubDoc;

use super::*;
use crate::ebook::test_support::fixture;
use crate::test_support::{build_stored_zip, build_test_epub};

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

#[test]
fn strip_tags_drops_script_and_style_bodies() {
    // `strip_tags` used to keep everything between the tags, so a stylesheet's
    // selectors and a script's identifiers counted as prose.
    let html = "<style>body { font-family: serif; margin: 0 }</style>\
                <p>Real words here.</p>\
                <script>var a = 1; function b() { return 2 }</script>";
    assert_eq!(strip_tags(html), "Real words here.");
}

#[test]
fn strip_tags_matches_the_suppressed_element_case_insensitively() {
    assert_eq!(strip_tags("<STYLE>p{}</STYLE>after"), "after");
    assert_eq!(strip_tags("<Script>x</Script>after"), "after");
}

#[test]
fn strip_tags_keeps_the_document_after_a_self_closing_script_tag() {
    // Treating `<script/>` as an opener would swallow the rest of the book,
    // which is a far worse failure than counting a few extra tokens.
    assert_eq!(strip_tags("<script/><p>Kept.</p>"), "Kept.");
}

#[test]
fn strip_tags_keeps_text_inside_an_element_merely_named_like_one() {
    // Prefix matching would suppress `<scriptorium>`; the element name is
    // compared whole.
    assert_eq!(strip_tags("<scriptorium>Kept</scriptorium>"), "Kept");
}

#[test]
fn strip_tags_ignores_comments_and_processing_instructions() {
    let html = "<?xml version=\"1.0\"?><!-- a note --><p>Body.</p>";
    assert_eq!(strip_tags(html), "Body.");
}

#[test]
fn estimate_word_count_skips_a_spine_document_declaring_itself_a_toc() {
    // An EPUB2 HTML table of contents is an ordinary spine item — nothing in
    // the manifest marks it — so its own `epub:type` is the only signal. Left
    // in, it charges the book once more for every chapter title it lists.
    let zip = build_test_epub(&[
        (
            "toc.xhtml",
            r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
               <nav epub:type="toc"><ol><li>Chapter One</li><li>Chapter Two</li></ol></nav>
               </body></html>"#,
        ),
        (
            "ch1.xhtml",
            "<html><body><p>One two three four.</p></body></html>",
        ),
    ]);
    let mut doc = EpubDoc::from_reader(std::io::Cursor::new(zip)).expect("parse in-memory epub");

    assert_eq!(estimate_word_count(&mut doc), Some(4));
}

#[test]
fn estimate_word_count_skips_the_manifest_marked_nav_document() {
    let zip = epub3_with_nav_in_spine();
    let mut doc = EpubDoc::from_reader(std::io::Cursor::new(zip)).expect("parse in-memory epub");
    // The nav document is in the spine here (EPUB3 permits it, and readers
    // that show a contents page rely on it), so the manifest's
    // `properties="nav"` is what keeps it out of the count.
    assert_eq!(doc.get_nav_id().as_deref(), Some("nav"));

    assert_eq!(estimate_word_count(&mut doc), Some(4));
}

#[test]
fn estimate_word_count_is_zero_for_a_spine_of_nothing_but_navigation() {
    // Zero, not `None`: the resources opened fine, so this is a measured
    // "no prose", which is a different fact from "couldn't read the book".
    let zip = build_test_epub(&[(
        "toc.xhtml",
        r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
           <nav epub:type="toc"><ol><li>Only a contents page</li></ol></nav>
           </body></html>"#,
    )]);
    let mut doc = EpubDoc::from_reader(std::io::Cursor::new(zip)).expect("parse in-memory epub");

    assert_eq!(estimate_word_count(&mut doc), Some(0));
}

#[test]
fn declares_property_matches_one_token_of_a_space_separated_list() {
    assert!(declares_property(Some("scripted nav"), "nav"));
    assert!(!declares_property(Some("navigation"), "nav"));
    assert!(!declares_property(None, "nav"));
}

#[test]
fn is_navigation_document_accepts_either_quote_style() {
    assert!(is_navigation_document(r#"<nav epub:type="toc">"#));
    assert!(is_navigation_document(r#"<nav epub:type='landmarks'>"#));
    assert!(is_navigation_document(r#"<nav epub:type="page-list">"#));
    assert!(!is_navigation_document(r#"<section epub:type="chapter">"#));
}

/// An EPUB3 whose spine holds the `properties="nav"` document *and* one
/// chapter. `build_test_epub_with_nav` keeps its nav out of the spine, which is
/// the case this walk never sees; this is the case it has to skip.
fn epub3_with_nav_in_spine() -> Vec<u8> {
    let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="pub-id">test-epub</dc:identifier><dc:title>Test</dc:title>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="nav"/><itemref idref="ch1"/></spine>
</package>"#;
    // Deliberately carries no `epub:type`, so only the manifest property can
    // identify it — otherwise the content sniff would pass this test for it.
    let nav = br#"<html><body><ol><li>Chapter One</li><li>Chapter Two</li></ol></body></html>"#;
    let ch1 = br#"<html><body><p>One two three four.</p></body></html>"#;
    build_stored_zip(&[
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", container),
        ("content.opf", opf),
        ("nav.xhtml", nav),
        ("ch1.xhtml", ch1),
    ])
}
