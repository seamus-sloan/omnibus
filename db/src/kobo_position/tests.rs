//! Unit tests for the span↔CFI translation. Fixtures are hand-authored
//! source/kepub pairs written to a temp dir — no kepubify binary involved;
//! the kepub variants mimic its output shape (koboSpan wrappers inside
//! `book-columns`/`book-inner` divs) with known text offsets.

use std::path::PathBuf;

use super::cfi::CfiTail;
use super::*;
use crate::test_support::{build_test_epub, build_test_kepub, make_test_dir};

const SOURCE_C1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body>
  <h1>Chapter One</h1>
  <p>First sentence here. Second sentence follows.</p>
  <p>Another paragraph with <em>emphasis</em> inside it.</p>
</body>
</html>"#;

const KEPUB_C1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body>
  <div id="book-columns"><div id="book-inner">
  <h1><span class="koboSpan" id="kobo.1.1">Chapter One</span></h1>
  <p><span class="koboSpan" id="kobo.2.1">First sentence here. </span><span class="koboSpan" id="kobo.2.2">Second sentence follows.</span></p>
  <p><span class="koboSpan" id="kobo.3.1">Another paragraph with </span><em><span class="koboSpan" id="kobo.3.2">emphasis</span></em><span class="koboSpan" id="kobo.3.3"> inside it.</span></p>
  </div></div>
</body>
</html>"#;

/// Write a one-chapter source/kepub pair; returns their paths.
fn fixture_pair(tag: &str, source_xhtml: &str, kepub_xhtml: &str) -> (PathBuf, PathBuf) {
    let dir = make_test_dir(&format!("kobo_position_{tag}"));
    let source = dir.join("book.epub");
    let kepub = dir.join("book.kepub.epub");
    std::fs::write(&source, build_test_epub(&[("c1.xhtml", source_xhtml)])).unwrap();
    std::fs::write(&kepub, build_test_kepub(&[("c1.xhtml", kepub_xhtml)])).unwrap();
    (source, kepub)
}

fn loc(source: &str, n: u32, m: u32) -> KoboLoc {
    KoboLoc {
        source: source.to_string(),
        n,
        m,
    }
}

// --- location.rs -------------------------------------------------------

#[test]
fn parse_location_extracts_source_and_counters_for_kobospan() {
    let parsed =
        parse_location(r#"{"Source":"text/part0009.html","Type":"KoboSpan","Value":"kobo.16.1"}"#);
    assert_eq!(parsed, Some(loc("text/part0009.html", 16, 1)));
}

#[test]
fn parse_location_rejects_non_kobospan_types_and_malformed_values() {
    assert_eq!(
        parse_location(r#"{"Source":"c1.xhtml","Type":"KoboAfCfi","Value":"kobo.1.1"}"#),
        None
    );
    assert_eq!(
        parse_location(r#"{"Source":"c1.xhtml","Type":"KoboSpan","Value":"1.1"}"#),
        None
    );
    assert_eq!(
        parse_location(r#"{"Source":"c1.xhtml","Type":"KoboSpan","Value":"kobo.x.1"}"#),
        None
    );
    assert_eq!(
        parse_location(r#"{"Type":"KoboSpan","Value":"kobo.1.1"}"#),
        None
    );
    assert_eq!(parse_location("not json"), None);
}

#[test]
fn location_json_round_trips_through_parse_location() {
    let json = location_json("text/ch1.xhtml", 4, 2);
    assert_eq!(parse_location(&json), Some(loc("text/ch1.xhtml", 4, 2)));
}

// --- cfi.rs ------------------------------------------------------------

#[test]
fn parse_cfi_maps_spine_step_22_to_index_10() {
    let cfi = parse_cfi("epubcfi(/6/22!/4/6/2/1:0)").expect("parse");
    assert_eq!(cfi.spine_index, 10);
    assert_eq!(cfi.tail.element_steps, vec![4, 6, 2]);
    assert_eq!(cfi.tail.text_index, 1);
    assert_eq!(cfi.tail.offset_utf16, 0);
}

#[test]
fn format_cfi_round_trips_through_parse_cfi() {
    let tail = CfiTail {
        element_steps: vec![4, 8],
        text_index: 3,
        offset_utf16: 17,
    };
    let formatted = format_cfi(4, &tail);
    assert_eq!(formatted, "epubcfi(/6/10!/4/8/3:17)");
    let parsed = parse_cfi(&formatted).expect("parse");
    assert_eq!(parsed.spine_index, 4);
    assert_eq!(parsed.tail, tail);
}

#[test]
fn parse_cfi_strips_id_assertions() {
    let cfi = parse_cfi("epubcfi(/6/22[part0009]!/4/6[chapter]/2/1:5)").expect("parse");
    assert_eq!(cfi.spine_index, 10);
    assert_eq!(cfi.tail.element_steps, vec![4, 6, 2]);
    assert_eq!(cfi.tail.offset_utf16, 5);
}

#[test]
fn parse_cfi_rejects_ranges_and_spatiotemporal_terms() {
    assert_eq!(parse_cfi("epubcfi(/6/22!/4/6,/2/1:0,/2/1:10)"), None);
    assert_eq!(parse_cfi("epubcfi(/6/22!/4/6/2~3.5)"), None);
    assert_eq!(parse_cfi("epubcfi(/6/22!/4/6/2@50:50)"), None);
    assert_eq!(parse_cfi("garbage"), None);
}

#[test]
fn parse_cfi_defaults_missing_offset_and_element_anchors() {
    let no_offset = parse_cfi("epubcfi(/6/2!/4/4/1)").expect("parse");
    assert_eq!(no_offset.tail.offset_utf16, 0);
    let element_anchor = parse_cfi("epubcfi(/6/2!/4/6)").expect("parse");
    assert_eq!(element_anchor.tail.element_steps, vec![4, 6]);
    assert_eq!(element_anchor.tail.text_index, 1);
    assert_eq!(element_anchor.tail.offset_utf16, 0);
}

// --- span_to_cfi -------------------------------------------------------

#[test]
fn span_to_cfi_maps_first_span_of_a_paragraph_to_offset_zero() {
    let (source, kepub) = fixture_pair("p1", SOURCE_C1, KEPUB_C1);
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 2, 1)).expect("derive");
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/4/1:0)"));
}

#[test]
fn span_to_cfi_maps_mid_node_span_to_utf16_offset() {
    let (source, kepub) = fixture_pair("mid", SOURCE_C1, KEPUB_C1);
    // kobo.2.2 starts at "Second", 21 code units into the paragraph's
    // single text node ("First sentence here. " precedes it).
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 2, 2)).expect("derive");
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/4/1:21)"));
}

#[test]
fn span_to_cfi_descends_into_nested_inline_markup() {
    let (source, kepub) = fixture_pair("em", SOURCE_C1, KEPUB_C1);
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 3, 2)).expect("derive");
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/6/2/1:0)"));
}

#[test]
fn span_to_cfi_lands_on_first_visible_char_after_leading_whitespace() {
    let (source, kepub) = fixture_pair("tail", SOURCE_C1, KEPUB_C1);
    // kobo.3.3 is " inside it." — the anchor is the "i", one code unit into
    // the paragraph's third text node (after the em).
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 3, 3)).expect("derive");
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/6/3:1)"));
}

#[test]
fn span_to_cfi_decodes_entities_identically_on_both_sides() {
    let source = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>Fish &amp; chips &#8212; the anchor follows.</p><p>Target here.</p></body></html>"#;
    // Same inter-element whitespace as the source (kepubify preserves it
    // verbatim); the em-dash is the decoded literal, as kepubify emits it.
    let kepub = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><div id="book-columns"><div id="book-inner"><p><span class="koboSpan" id="kobo.1.1">Fish &amp; chips — the anchor follows.</span></p><p><span class="koboSpan" id="kobo.2.1">Target here.</span></p></div></div></body></html>"#;
    let (source, kepub) = fixture_pair("ent", source, kepub);
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 2, 1)).expect("derive");
    // The em-dash is one scalar on both sides, so the second paragraph's
    // anchor still lands at its own text node start.
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/4/1:0)"));
}

#[test]
fn span_to_cfi_counts_utf16_units_for_astral_chars_before_the_anchor() {
    let source = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>A 😀 b anchor.</p></body></html>"#;
    let kepub = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p><span class="koboSpan" id="kobo.1.1">A 😀 b </span><span class="koboSpan" id="kobo.1.2">anchor.</span></p></body></html>"#;
    let (source, kepub) = fixture_pair("emoji", source, kepub);
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 1, 2)).expect("derive");
    // "A 😀 b " is 6 scalars but 7 UTF-16 units — the offset must count
    // units or epub.js lands one char short.
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/2/1:7)"));
}

#[test]
fn span_to_cfi_splits_text_index_across_br_boundaries() {
    let source = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>One line<br/>second line here.</p></body></html>"#;
    let kepub = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p><span class="koboSpan" id="kobo.1.1">One line</span><br/><span class="koboSpan" id="kobo.1.2">second line here.</span></p></body></html>"#;
    let (source, kepub) = fixture_pair("br", source, kepub);
    let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", 1, 2)).expect("derive");
    // The text after <br/> is the paragraph's second text node: index 3.
    assert_eq!(cfi.as_deref(), Some("epubcfi(/6/2!/4/2/3:0)"));
}

#[test]
fn span_to_cfi_returns_none_for_missing_span_or_unknown_source() {
    let (source, kepub) = fixture_pair("miss", SOURCE_C1, KEPUB_C1);
    assert_eq!(
        span_to_cfi(&kepub, &source, &loc("c1.xhtml", 9, 9)).expect("derive"),
        None
    );
    assert_eq!(
        span_to_cfi(&kepub, &source, &loc("nope.xhtml", 1, 1)).expect("derive"),
        None
    );
}

#[test]
fn span_to_cfi_returns_none_when_kepub_text_diverges_from_source() {
    // The kepub claims different prose than the source file — a replaced
    // source under a stale cache. The snippet check must refuse to map.
    let kepub = KEPUB_C1.replace("First sentence here.", "Entirely different text.");
    let (source, kepub) = fixture_pair("diverge", SOURCE_C1, &kepub);
    assert_eq!(
        span_to_cfi(&kepub, &source, &loc("c1.xhtml", 2, 1)).expect("derive"),
        None
    );
}

// --- cfi_to_span -------------------------------------------------------

#[test]
fn cfi_to_span_finds_the_span_containing_a_mid_sentence_cfi() {
    let (source, kepub) = fixture_pair("rev1", SOURCE_C1, KEPUB_C1);
    let derived = cfi_to_span(Some(&kepub), &source, "epubcfi(/6/2!/4/4/1:5)").expect("derive");
    // Offset 5 is inside "First sentence here. " → kobo.2.1.
    assert_eq!(
        derived.location_json.as_deref(),
        Some(location_json("c1.xhtml", 2, 1).as_str())
    );
    assert!(derived.percent.is_some());
}

#[test]
fn cfi_to_span_maps_a_second_sentence_cfi_to_its_own_span() {
    let (source, kepub) = fixture_pair("rev2", SOURCE_C1, KEPUB_C1);
    let derived = cfi_to_span(Some(&kepub), &source, "epubcfi(/6/2!/4/4/1:21)").expect("derive");
    assert_eq!(
        derived.location_json.as_deref(),
        Some(location_json("c1.xhtml", 2, 2).as_str())
    );
}

#[test]
fn cfi_to_span_resolves_element_anchored_cfis_to_the_first_span_within() {
    let (source, kepub) = fixture_pair("rev3", SOURCE_C1, KEPUB_C1);
    let derived = cfi_to_span(Some(&kepub), &source, "epubcfi(/6/2!/4/6)").expect("derive");
    assert_eq!(
        derived.location_json.as_deref(),
        Some(location_json("c1.xhtml", 3, 1).as_str())
    );
}

#[test]
fn cfi_to_span_clamps_past_the_last_span_to_the_last_span() {
    let (source, kepub) = fixture_pair("rev4", SOURCE_C1, KEPUB_C1);
    let derived = cfi_to_span(Some(&kepub), &source, "epubcfi(/6/2!/4/6/3:200)").expect("derive");
    assert_eq!(
        derived.location_json.as_deref(),
        Some(location_json("c1.xhtml", 3, 3).as_str())
    );
}

#[test]
fn cfi_to_span_without_kepub_still_yields_percent() {
    let (source, _kepub) = fixture_pair("rev5", SOURCE_C1, KEPUB_C1);
    let derived = cfi_to_span(None, &source, "epubcfi(/6/2!/4/4/1:0)").expect("derive");
    assert_eq!(derived.location_json, None);
    // "First" starts at 12 of 101 visible chars → 11%.
    assert_eq!(derived.percent, Some(11));
}

#[test]
fn cfi_to_span_computes_percent_across_multiple_spine_files() {
    let dir = make_test_dir("kobo_position_multifile");
    let filler = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>0123456789012345678901234</p></body></html>"#; // 25 visible chars
    let source_path = dir.join("book.epub");
    std::fs::write(
        &source_path,
        build_test_epub(&[
            ("c0.xhtml", filler),
            ("c1.xhtml", filler),
            ("c2.xhtml", filler),
            ("c3.xhtml", filler),
        ]),
    )
    .unwrap();
    // Anchor at the start of the third file: 50 of 100 chars → 50%.
    let derived = cfi_to_span(None, &source_path, "epubcfi(/6/6!/4/2/1:0)").expect("derive");
    assert_eq!(derived.percent, Some(50));
}

#[test]
fn cfi_to_span_returns_empty_for_unparseable_or_out_of_range_cfis() {
    let (source, kepub) = fixture_pair("rev6", SOURCE_C1, KEPUB_C1);
    let bad = cfi_to_span(Some(&kepub), &source, "not a cfi").expect("derive");
    assert_eq!(bad.location_json, None);
    assert_eq!(bad.percent, None);
    let out_of_range =
        cfi_to_span(Some(&kepub), &source, "epubcfi(/6/40!/4/2/1:0)").expect("derive");
    assert_eq!(out_of_range.percent, None);
}

// --- round trip + golden vector ---------------------------------------

#[test]
fn span_round_trips_through_cfi_and_back() {
    let (source, kepub) = fixture_pair("round", SOURCE_C1, KEPUB_C1);
    for (n, m) in [(1u32, 1u32), (2, 1), (2, 2), (3, 1), (3, 2), (3, 3)] {
        let cfi = span_to_cfi(&kepub, &source, &loc("c1.xhtml", n, m))
            .expect("derive")
            .expect("cfi");
        let derived = cfi_to_span(Some(&kepub), &source, &cfi).expect("derive back");
        assert_eq!(
            derived.location_json.as_deref(),
            Some(location_json("c1.xhtml", n, m).as_str()),
            "round trip for kobo.{n}.{m} via {cfi}"
        );
    }
}

#[test]
fn span_to_cfi_emits_spine_step_22_for_the_eleventh_spine_item() {
    // Golden-vector shape from production: span kobo.16.1 in the 11th
    // spine item (text/part0009.html) maps to an /6/22!… CFI.
    let dir = make_test_dir("kobo_position_golden");
    let filler = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>filler</p></body></html>"#;
    let mut paragraphs = String::new();
    let mut kepub_paragraphs = String::new();
    for i in 1..=16 {
        paragraphs.push_str(&format!("<p>Paragraph number {i} sits here.</p>"));
        kepub_paragraphs.push_str(&format!(
            r#"<p><span class="koboSpan" id="kobo.{i}.1">Paragraph number {i} sits here.</span></p>"#
        ));
    }
    let chapter = format!(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body>{paragraphs}</body></html>"#
    );
    let kepub_chapter = format!(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body>{kepub_paragraphs}</body></html>"#
    );
    let mut spine: Vec<(String, String)> = (0..10)
        .map(|i| (format!("text/part{i:04}.html"), filler.to_string()))
        .collect();
    spine.push(("text/part0009x.html".into(), chapter.clone()));
    // Rename so the 11th item is the conventional production name.
    spine[10].0 = "text/part0010.html".into();
    let entries: Vec<(&str, &str)> = spine
        .iter()
        .map(|(h, x)| (h.as_str(), x.as_str()))
        .collect();
    let source_path = dir.join("book.epub");
    std::fs::write(&source_path, build_test_epub(&entries)).unwrap();
    let kepub_entries: Vec<(String, String)> = spine
        .iter()
        .enumerate()
        .map(|(i, (h, x))| {
            let body = if i == 10 {
                kepub_chapter.clone()
            } else {
                x.clone()
            };
            (h.clone(), body)
        })
        .collect();
    let kepub_refs: Vec<(&str, &str)> = kepub_entries
        .iter()
        .map(|(h, x)| (h.as_str(), x.as_str()))
        .collect();
    let kepub_path = dir.join("book.kepub.epub");
    std::fs::write(&kepub_path, build_test_kepub(&kepub_refs)).unwrap();

    let cfi = span_to_cfi(&kepub_path, &source_path, &loc("text/part0010.html", 16, 1))
        .expect("derive")
        .expect("cfi");
    // 16th paragraph = body element child 16 → step 32; spine index 10 → /6/22.
    assert_eq!(cfi, "epubcfi(/6/22!/4/32/1:0)");
    let derived = cfi_to_span(Some(&kepub_path), &source_path, &cfi).expect("reverse");
    assert_eq!(
        derived.location_json.as_deref(),
        Some(location_json("text/part0010.html", 16, 1).as_str())
    );
}
