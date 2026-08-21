//! Tests for EPUB OPF metadata parsing: refinement lookup, contributor
//! role/file-as extraction across EPUB3 refinements and legacy attributes,
//! and series extraction from both the EPUB3 `belongs-to-collection` form
//! and the legacy Calibre series meta.

use std::io::Cursor;

use epub::doc::{EpubDoc, MetadataRefinement};

use super::{collect_contributors, collect_identifiers, collect_series, first, lookup_refinement};
use crate::ebook::scan_ebook_library;
use crate::ebook::test_support::*;
use crate::test_support::build_stored_zip;

// --- In-memory OPF harness ---------------------------------------------
//
// The `collect_*`/`first` helpers only read `doc.metadata`, but `EpubDoc`
// (and the `MetadataItem`s it holds) can't be constructed directly — the
// `id` field is private and the only constructors want a real zip. So we
// wrap a hand-built OPF in the smallest valid EPUB container and let the
// real parser produce the `metadata` vec, exercising the exact
// refinement/attribute branches disk fixtures would. `MetadataRefinement`
// *is* fully public, so `lookup_refinement` is tested on it directly.

// The OCF spec requires the archive to open with an uncompressed
// `mimetype` entry holding exactly `application/epub+zip` (no trailing
// newline). `build_stored_zip` stores every entry, and `doc_from_opf` lists this
// first, so the fixtures are spec-complete rather than leaning on the
// `epub` crate's tolerance for a missing mimetype.
const MIMETYPE: &[u8] = b"application/epub+zip";

const CONTAINER_XML: &[u8] = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

// Minimal EPUB3 navigation document the OPF manifest references. Included
// so the manifest points at a real resource instead of a dangling href.
const NAV_XHTML: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <head><title>Contents</title></head>
  <body>
<nav epub:type="toc"><ol><li><a href="content.opf">Start</a></li></ol></nav>
  </body>
</html>"#;

/// Parse an in-memory EPUB whose sole OPF is `opf`. The manifest/spine are
/// required by the parser, so every fixture below wraps its `<metadata>`
/// in [`opf_package`]. The `mimetype` entry comes first per the OCF spec.
fn doc_from_opf(opf: &str) -> EpubDoc<Cursor<Vec<u8>>> {
    let zip = build_stored_zip(&[
        ("mimetype", MIMETYPE),
        ("META-INF/container.xml", CONTAINER_XML),
        ("content.opf", opf.as_bytes()),
        ("nav.xhtml", NAV_XHTML),
    ]);
    EpubDoc::from_reader(Cursor::new(zip)).expect("parse in-memory epub")
}

/// Wrap a `<metadata>` body in a full OPF package at the given version
/// (`"3.0"` selects the EPUB3 refinement branch; `"2.0"` the legacy branch
/// where `opf:*` attributes on `<dc:*>` become refinements).
fn opf_package(version: &str, metadata_body: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="{version}" unique-identifier="pub-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
{metadata_body}
  </metadata>
  <manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="nav"/></spine>
</package>"#
    )
}

// --- lookup_refinement -------------------------------------------------

#[test]
fn lookup_refinement_returns_value_for_matching_property() {
    let refs = vec![
        MetadataRefinement {
            property: "file-as".into(),
            value: "Le Guin, Ursula K.".into(),
            lang: None,
            scheme: None,
        },
        MetadataRefinement {
            property: "role".into(),
            value: "aut".into(),
            lang: None,
            scheme: None,
        },
    ];
    assert_eq!(lookup_refinement(&refs, "role"), Some("aut".to_string()));
}

#[test]
fn lookup_refinement_returns_none_when_property_absent() {
    let refs = vec![MetadataRefinement {
        property: "role".into(),
        value: "aut".into(),
        lang: None,
        scheme: None,
    }];
    assert_eq!(lookup_refinement(&refs, "file-as"), None);
    assert_eq!(lookup_refinement(&[], "role"), None);
}

#[test]
fn lookup_refinement_returns_first_match_when_duplicated() {
    let refs = vec![
        MetadataRefinement {
            property: "scheme".into(),
            value: "uuid".into(),
            lang: None,
            scheme: None,
        },
        MetadataRefinement {
            property: "scheme".into(),
            value: "isbn".into(),
            lang: None,
            scheme: None,
        },
    ];
    assert_eq!(lookup_refinement(&refs, "scheme"), Some("uuid".to_string()));
}

// --- collect_contributors ----------------------------------------------

#[test]
fn collect_contributors_reads_role_and_file_as_from_epub3_refinements() {
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r##"    <dc:creator id="aut">Ursula K. Le Guin</dc:creator>
<meta refines="#aut" property="role">aut</meta>
<meta refines="#aut" property="file-as">Le Guin, Ursula K.</meta>"##,
    ));
    let creators = collect_contributors(&doc, "creator");
    assert_eq!(creators.len(), 1);
    assert_eq!(creators[0].name, "Ursula K. Le Guin");
    assert_eq!(creators[0].role.as_deref(), Some("aut"));
    assert_eq!(creators[0].file_as.as_deref(), Some("Le Guin, Ursula K."));
}

#[test]
fn collect_contributors_reads_role_and_file_as_from_legacy_opf_attributes() {
    // Legacy EPUB2: `opf:role` / `opf:file-as` ride as attributes on the
    // `<dc:creator>` element rather than separate `<meta refines>` nodes.
    let doc = doc_from_opf(&opf_package(
        "2.0",
        r#"    <dc:creator opf:role="aut" opf:file-as="Le Guin, Ursula K.">Ursula K. Le Guin</dc:creator>"#,
    ));
    let creators = collect_contributors(&doc, "creator");
    assert_eq!(creators.len(), 1);
    assert_eq!(creators[0].name, "Ursula K. Le Guin");
    assert_eq!(creators[0].role.as_deref(), Some("aut"));
    assert_eq!(creators[0].file_as.as_deref(), Some("Le Guin, Ursula K."));
}

#[test]
fn collect_contributors_filters_by_key_and_skips_blank_names() {
    // Only the requested property is returned, and whitespace-only names
    // are dropped. `contributor` is selected here to prove keying works
    // for the second role the pipeline merges in.
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r"    <dc:creator>Primary Author</dc:creator>
<dc:contributor>   </dc:contributor>
<dc:contributor>Jane Editor</dc:contributor>",
    ));
    let contributors = collect_contributors(&doc, "contributor");
    assert_eq!(contributors.len(), 1);
    assert_eq!(contributors[0].name, "Jane Editor");
    assert!(contributors[0].role.is_none());
    assert!(contributors[0].file_as.is_none());
}

#[test]
fn collect_contributors_drops_bkp_role_conversion_tool_stamp() {
    // Calibre stamps its generator string as a `bkp` (book producer)
    // contributor; it must never surface as an author. Legacy EPUB2
    // attribute form, the shape Calibre actually writes.
    let doc = doc_from_opf(&opf_package(
        "2.0",
        r#"    <dc:contributor opf:role="bkp">calibre (2.80.0) [https://calibre-ebook.com]</dc:contributor>
<dc:contributor opf:role="trl">Ken Liu</dc:contributor>"#,
    ));
    let contributors = collect_contributors(&doc, "contributor");
    assert_eq!(contributors.len(), 1);
    assert_eq!(contributors[0].name, "Ken Liu");
    assert_eq!(contributors[0].role.as_deref(), Some("trl"));
}

#[test]
fn collect_contributors_drops_bkp_role_from_epub3_refinement() {
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r##"    <dc:creator>Cixin Liu</dc:creator>
<dc:contributor id="bkp">calibre (7.3.0) [https://calibre-ebook.com]</dc:contributor>
<meta refines="#bkp" property="role">bkp</meta>"##,
    ));
    assert_eq!(collect_contributors(&doc, "contributor").len(), 0);
    // The real creator is untouched by the filter.
    let creators = collect_contributors(&doc, "creator");
    assert_eq!(creators.len(), 1);
    assert_eq!(creators[0].name, "Cixin Liu");
}

// --- collect_series ----------------------------------------------------

#[test]
fn collect_series_reads_epub3_belongs_to_collection_with_group_position() {
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r##"    <meta property="belongs-to-collection" id="c1">Earthsea</meta>
<meta refines="#c1" property="group-position">2</meta>"##,
    ));
    let (series, index) = collect_series(&doc);
    assert_eq!(series.as_deref(), Some("Earthsea"));
    assert_eq!(index.as_deref(), Some("2"));
}

#[test]
fn collect_series_falls_back_to_legacy_calibre_series_meta() {
    // No `belongs-to-collection`, so the Calibre `<meta name="...">` keys
    // supply both the name and the index.
    let doc = doc_from_opf(&opf_package(
        "2.0",
        r#"    <meta name="calibre:series" content="Earthsea"/>
<meta name="calibre:series_index" content="2.0"/>"#,
    ));
    let (series, index) = collect_series(&doc);
    assert_eq!(series.as_deref(), Some("Earthsea"));
    assert_eq!(index.as_deref(), Some("2.0"));
}

#[test]
fn collect_series_returns_none_when_no_series_metadata_present() {
    let doc = doc_from_opf(&opf_package("3.0", r"    <dc:title>Standalone</dc:title>"));
    let (series, index) = collect_series(&doc);
    assert_eq!(series, None);
    assert_eq!(index, None);
}

// --- collect_identifiers -----------------------------------------------

#[test]
fn collect_identifiers_reads_scheme_from_epub3_refinement() {
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r##"    <dc:identifier id="pub-id">urn:uuid:abc</dc:identifier>
<meta refines="#pub-id" property="scheme">uuid</meta>"##,
    ));
    let ids = collect_identifiers(&doc);
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].value, "urn:uuid:abc");
    assert_eq!(ids[0].scheme.as_deref(), Some("uuid"));
}

#[test]
fn collect_identifiers_reads_scheme_from_legacy_opf_scheme_attribute() {
    // Legacy EPUB2 carries the scheme as an `opf:scheme` attribute, which
    // the parser surfaces as a `scheme` refinement.
    let doc = doc_from_opf(&opf_package(
        "2.0",
        r#"    <dc:identifier id="pub-id" opf:scheme="ISBN">9780000000000</dc:identifier>"#,
    ));
    let ids = collect_identifiers(&doc);
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].value, "9780000000000");
    assert_eq!(ids[0].scheme.as_deref(), Some("ISBN"));
}

#[test]
fn collect_identifiers_leaves_scheme_none_when_unspecified_and_skips_blanks() {
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r"    <dc:identifier>plain-id</dc:identifier>
<dc:identifier>   </dc:identifier>",
    ));
    let ids = collect_identifiers(&doc);
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].value, "plain-id");
    assert!(ids[0].scheme.is_none());
}

// --- first (shared refinement-adjacent reader) -------------------------

#[test]
fn first_returns_trimmed_value_for_present_key_and_none_otherwise() {
    let doc = doc_from_opf(&opf_package(
        "3.0",
        r"    <dc:title>  Earthsea  </dc:title>",
    ));
    assert_eq!(first(&doc, "title").as_deref(), Some("Earthsea"));
    assert_eq!(first(&doc, "publisher"), None);
}

#[test]
fn scan_records_parse_errors_per_file() {
    let dir = make_test_dir("bad");
    std::fs::write(dir.join("broken.epub"), b"not actually a zip").unwrap();
    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.books.len(), 1);
    assert!(out.books[0].metadata.error.is_some());
    assert_eq!(out.books[0].metadata.filename, "broken.epub");
}

#[test]
fn scan_handles_calibre_shaped_tree_and_ignores_metadata_opf() {
    // Lock in the read-tolerance promise from F0.6: a Calibre-style
    // library tree (`<Lastname, First>/Title (id)/title.epub` plus an
    // adjacent `metadata.opf` Calibre wrote out) must scan correctly.
    // We assert (a) the epub is found, (b) the title comes from the
    // *embedded* OPF inside the epub, not the deliberately-wrong
    // sidecar `metadata.opf`. The sidecar is ignored entirely.
    let dir = make_test_dir("calibre_shaped");
    let book_dir = dir.join("Lovelace, Ada").join("Alpha (42)");
    std::fs::create_dir_all(&book_dir).unwrap();
    std::fs::copy(fixture("alpha.epub"), book_dir.join("alpha.epub")).unwrap();
    // Calibre's metadata.opf — write garbage into it so any code path
    // that *did* read it would visibly disagree with the embedded OPF.
    std::fs::write(
        book_dir.join("metadata.opf"),
        br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>WRONG TITLE FROM CALIBRE SIDECAR</dc:title>
<dc:creator>Wrong Author</dc:creator>
</metadata></package>"#,
    )
    .unwrap();

    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(out.error.is_none(), "scan errored: {:?}", out.error);
    // Exactly one epub found despite the metadata.opf sibling.
    let epubs: Vec<_> = out
        .books
        .iter()
        .filter(|b| b.metadata.error.is_none())
        .collect();
    assert_eq!(epubs.len(), 1);
    // Title comes from the embedded OPF, not the misleading sidecar.
    assert_eq!(epubs[0].metadata.title.as_deref(), Some("Alpha"));
    // Word count is estimated from the spine at parse time — alpha.epub's
    // sole spine item is 4 whitespace-separated tokens (see
    // `ebook::wordcount::tests`).
    assert_eq!(epubs[0].word_count, Some(4));
}
