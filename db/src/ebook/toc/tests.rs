//! Unit tests for EPUB structure extraction: spine stats, nav-first TOC
//! with NCX fallback, href resolution edges, and the honest no-TOC case.

use std::io::Cursor;

use epub::doc::EpubDoc;

use crate::test_support::{build_test_epub, build_test_epub_with_nav, build_test_epub_with_ncx};

use super::*;

const CH: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C</title></head>
<body><p>First sentence here. Second sentence follows.</p></body>
</html>"#;

fn open(bytes: Vec<u8>) -> EpubDoc<Cursor<Vec<u8>>> {
    EpubDoc::from_reader(Cursor::new(bytes)).unwrap()
}

const SPINE3: &[(&str, &str)] = &[("c1.xhtml", CH), ("c2.xhtml", CH), ("c3.xhtml", CH)];

#[test]
fn extract_structure_measures_spine_and_reads_nav_toc() {
    let mut doc = open(build_test_epub_with_nav(
        SPINE3,
        &[("Chapter One", "c1.xhtml"), ("Chapter Three", "c3.xhtml")],
    ));
    let s = extract_structure(&mut doc).unwrap();

    assert_eq!(s.spine.len(), 3);
    let chars = s.spine[0].visible_chars;
    assert!(chars > 0, "identical chapters must measure non-empty");
    assert!(s.spine.iter().all(|x| x.visible_chars == chars));
    assert_eq!(s.spine[1].href, "c2.xhtml");

    assert_eq!(s.chapters.len(), 2);
    assert_eq!(s.chapters[0].title, "Chapter One");
    assert_eq!(
        (s.chapters[0].spine_index, s.chapters[0].start_chars),
        (0, 0)
    );
    assert_eq!(s.chapters[1].title, "Chapter Three");
    assert_eq!(
        (s.chapters[1].spine_index, s.chapters[1].start_chars),
        (2, 2 * chars)
    );
    assert_eq!(s.chapters[1].ordinal, 1);
}

#[test]
fn extract_structure_falls_back_to_ncx_when_no_nav() {
    let mut doc = open(build_test_epub_with_ncx(
        SPINE3,
        &[("Opening", "c1.xhtml"), ("Middle", "c2.xhtml")],
    ));
    let s = extract_structure(&mut doc).unwrap();
    assert_eq!(s.chapters.len(), 2);
    assert_eq!(s.chapters[0].title, "Opening");
    assert_eq!(s.chapters[1].title, "Middle");
    assert_eq!(s.chapters[1].spine_index, 1);
    assert_eq!(s.chapters[1].start_chars, s.spine[0].visible_chars);
}

#[test]
fn extract_structure_returns_empty_chapters_for_a_tocless_epub() {
    // The honest no-TOC case: stats extracted, zero chapters — never a
    // fabricated chapter list.
    let mut doc = open(build_test_epub(SPINE3));
    let s = extract_structure(&mut doc).unwrap();
    assert_eq!(s.spine.len(), 3);
    assert!(s.chapters.is_empty());
}

#[test]
fn extract_structure_keeps_fragment_hrefs_and_resolves_their_spine_item() {
    let mut doc = open(build_test_epub_with_nav(
        SPINE3,
        &[("Part Two", "c2.xhtml#intro")],
    ));
    let s = extract_structure(&mut doc).unwrap();
    assert_eq!(s.chapters.len(), 1);
    assert_eq!(s.chapters[0].href, "c2.xhtml#intro");
    assert_eq!(s.chapters[0].spine_index, 1);
}

#[test]
fn extract_structure_drops_entries_that_resolve_to_no_spine_item() {
    let mut doc = open(build_test_epub_with_nav(
        SPINE3,
        &[("Ghost", "missing.xhtml"), ("Real", "c1.xhtml")],
    ));
    let s = extract_structure(&mut doc).unwrap();
    assert_eq!(s.chapters.len(), 1);
    assert_eq!(s.chapters[0].title, "Real");
    assert_eq!(s.chapters[0].ordinal, 0);
}

#[test]
fn spine_index_for_never_matches_mid_segment() {
    let spine = vec![SpineStat {
        spine_index: 0,
        href: "text/chapter11.xhtml".into(),
        visible_chars: 10,
    }];
    // "1.xhtml" is a mid-segment suffix of "chapter11.xhtml" — must not
    // match; only whole segments align.
    assert_eq!(spine_index_for(&spine, "1.xhtml"), None);
    assert_eq!(spine_index_for(&spine, "chapter11.xhtml"), Some(0));
    assert_eq!(spine_index_for(&spine, "text/chapter11.xhtml"), Some(0));
}

#[test]
fn join_href_resolves_dot_segments_and_keeps_fragments() {
    assert_eq!(join_href("xhtml", "../text/c1.xhtml#f"), "text/c1.xhtml#f");
    assert_eq!(join_href("", "c1.xhtml"), "c1.xhtml");
    assert_eq!(join_href("a/b", "./c.xhtml"), "a/b/c.xhtml");
}
