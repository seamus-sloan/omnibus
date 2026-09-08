//! The two pure transforms: `transform_opf` replacing the managed fields
//! (escaping, control-char stripping, absent values dropped, unbound `dc`
//! namespace errors) and `encode_cover_for` transcoding or passing through
//! the override cover.

use image::ImageFormat;
use omnibus_shared::{Contributor, EbookMetadata};

use super::super::cover::encode_cover_for;
use super::super::opf::transform_opf;
use super::{png, SAMPLE_OPF};

/// Effective metadata standing in for a fully-overridden book.
fn overridden_book() -> EbookMetadata {
    EbookMetadata {
        id: 1,
        filename: "book.epub".into(),
        title: Some("New Title".into()),
        creators: vec![Contributor {
            name: "New Author".into(),
            role: Some("aut".into()),
            file_as: Some("Author, New".into()),
            id: None,
        }],
        subjects: vec!["NewSubj".into()],
        series: Some("New Series".into()),
        series_index: Some("1".into()),
        language: Some("en".into()),
        ..Default::default()
    }
}

/// Assert `xml` parses to EOF without a quick-xml error (well-formedness).
fn assert_well_formed(xml: &[u8]) {
    let mut reader = quick_xml::reader::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("rewritten OPF is not well-formed: {e}"),
        }
        buf.clear();
    }
}

#[test]
fn transform_opf_replaces_managed_fields_and_preserves_structure() {
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &overridden_book()).unwrap();
    let s = String::from_utf8(out.clone()).unwrap();
    assert_well_formed(&out);

    // Managed descriptive fields are replaced.
    assert!(s.contains("<dc:title>New Title</dc:title>"), "{s}");
    assert!(!s.contains("Original Title"), "{s}");
    assert!(s.contains("New Author"), "{s}");
    assert!(!s.contains("Original Author"), "{s}");
    assert!(s.contains("<dc:subject>NewSubj</dc:subject>"), "{s}");
    assert!(!s.contains("OldSubject"), "{s}");
    assert!(
        s.contains(r#"<meta name="calibre:series" content="New Series"/>"#),
        "{s}"
    );
    assert!(!s.contains("Old Series"), "{s}");
    assert!(
        s.contains(r#"<meta name="calibre:series_index" content="1"/>"#),
        "{s}"
    );

    // Package identity, cover pointer, refinements, manifest, and spine survive.
    assert!(s.contains(r#"unique-identifier="bookid""#), "{s}");
    assert!(s.contains("urn:uuid:ORIG-UUID-9f3"), "{s}");
    assert!(
        s.contains(r#"<meta name="cover" content="cover-img"/>"#),
        "{s}"
    );
    assert!(s.contains("dcterms:modified"), "{s}");
    assert!(
        s.contains(r#"<item id="cover-img" href="cover.jpg" media-type="image/jpeg"/>"#),
        "{s}"
    );
    assert!(s.contains(r#"<itemref idref="nav"/>"#), "{s}");
}

#[test]
fn transform_opf_escapes_values_and_falls_back_to_filename_for_empty_title() {
    let book = EbookMetadata {
        filename: "fallback.epub".into(),
        title: None,
        publisher: Some("A & B <Publishing>".into()),
        ..Default::default()
    };
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &book).unwrap();
    let s = String::from_utf8(out.clone()).unwrap();
    assert_well_formed(&out);
    assert!(s.contains("<dc:title>fallback.epub</dc:title>"), "{s}");
    assert!(
        s.contains("<dc:publisher>A &amp; B &lt;Publishing&gt;</dc:publisher>"),
        "{s}"
    );
}

#[test]
fn transform_opf_escapes_quotes_and_apostrophes_in_creator_attributes() {
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("T".into()),
        creators: vec![Contributor {
            name: "A & B".into(),
            role: None,
            file_as: Some("O'Neil & \"Co\"".into()),
            id: None,
        }],
        ..Default::default()
    };
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &book).unwrap();
    let s = String::from_utf8(out.clone()).unwrap();
    assert_well_formed(&out);
    assert!(
        s.contains("opf:file-as=\"O&apos;Neil &amp; &quot;Co&quot;\""),
        "{s}"
    );
    assert!(s.contains(">A &amp; B</dc:creator>"), "{s}");
}

#[test]
fn transform_opf_strips_xml_illegal_control_chars_while_keeping_tab_lf_cr() {
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("Stray\u{1}Control".into()),
        description: Some("Tab\tKept\nLF\rCR\u{c}Stripped".into()),
        ..Default::default()
    };
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &book).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("<dc:title>StrayControl</dc:title>"), "{s}");
    assert!(
        s.contains("<dc:description>Tab\tKept\nLF\rCRStripped</dc:description>"),
        "{s}"
    );
}

#[test]
fn transform_opf_drops_managed_fields_when_effective_value_is_absent() {
    // An override that clears series/subjects: the originals must not linger.
    let book = EbookMetadata {
        filename: "x.epub".into(),
        title: Some("Only Title".into()),
        ..Default::default()
    };
    let out = transform_opf(SAMPLE_OPF.as_bytes(), &book).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(!s.contains("calibre:series"), "{s}");
    assert!(!s.contains("OldSubject"), "{s}");
    // But the cover pointer (an unmanaged meta) is still preserved.
    assert!(
        s.contains(r#"<meta name="cover" content="cover-img"/>"#),
        "{s}"
    );
}

#[test]
fn transform_opf_errors_when_dc_namespace_is_unbound() {
    // Metadata that never binds `xmlns:dc` — injecting `<dc:*>` would be invalid
    // XML, so the rewrite must fail and let the caller serve the source EPUB.
    let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata>
    <identifier id="id">book-1</identifier>
    <title>Original</title>
  </metadata>
  <manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="nav"/></spine>
</package>"#;
    let err = transform_opf(opf.as_bytes(), &overridden_book()).unwrap_err();
    assert!(
        err.to_string().contains("xmlns:dc"),
        "expected an unbound-dc error, got: {err}"
    );
}

#[test]
fn transform_opf_errors_when_dc_namespace_is_only_declared_on_a_managed_descendant() {
    // `<package>`/`<metadata>` never bind `xmlns:dc`, but the managed
    // `<dc:title>` child redundantly re-declares it on itself. That
    // declaration is out of scope again by the time we inject new `dc:*`
    // markup just before `</metadata>`, so it must not satisfy the
    // dc-bound check — the rewrite must still bail rather than emit markup
    // that relies on an unbound prefix.
    let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata>
    <identifier id="id">book-1</identifier>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Original</dc:title>
  </metadata>
  <manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="nav"/></spine>
</package>"#;
    let err = transform_opf(opf.as_bytes(), &overridden_book()).unwrap_err();
    assert!(
        err.to_string().contains("xmlns:dc"),
        "expected an unbound-dc error despite the redundant descendant declaration, got: {err}"
    );
}

#[test]
fn encode_cover_for_returns_bytes_unchanged_when_already_target_format() {
    let src = png(2, 2, [10, 20, 30]);
    let out = encode_cover_for("image/png", "image/png", src.clone()).unwrap();
    assert_eq!(out, src);
}

#[test]
fn encode_cover_for_transcodes_to_target_format() {
    let src = png(4, 4, [200, 100, 50]);
    let out = encode_cover_for("image/jpeg", "image/png", src).unwrap();
    assert_eq!(
        image::guess_format(&out).unwrap(),
        ImageFormat::Jpeg,
        "output should be JPEG"
    );
}

#[test]
fn encode_cover_for_returns_none_for_undecodable_override() {
    assert!(encode_cover_for("image/jpeg", "image/png", b"not an image".to_vec()).is_none());
}

#[test]
fn encode_cover_for_returns_none_for_unencodable_target_format() {
    let src = png(2, 2, [0, 0, 0]);
    assert!(encode_cover_for("image/svg+xml", "image/png", src).is_none());
}
