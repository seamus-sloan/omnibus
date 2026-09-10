use super::*;
use crate::epub_structure::{EbookChapterRow, SpineStatRow};

/// A book whose three spine documents measure 100 / 300 / 100 chars, with a
/// TOC entry opening each.
fn index() -> AnchorIndex {
    AnchorIndex {
        spine: vec![
            SpineStatRow {
                spine_index: 0,
                href: "a.xhtml".into(),
                visible_chars: 100,
                chars_before: 0,
            },
            SpineStatRow {
                spine_index: 1,
                href: "b.xhtml".into(),
                visible_chars: 300,
                chars_before: 100,
            },
            SpineStatRow {
                spine_index: 2,
                href: "c.xhtml".into(),
                visible_chars: 100,
                chars_before: 400,
            },
        ],
        chapters: vec![
            EbookChapterRow {
                ordinal: 0,
                title: "One".into(),
                href: "a.xhtml".into(),
                spine_index: 0,
                start_chars: 0,
            },
            EbookChapterRow {
                ordinal: 1,
                title: "Two".into(),
                href: "b.xhtml".into(),
                spine_index: 1,
                start_chars: 100,
            },
        ],
        total_chars: 500,
        audio_seconds: Some(1_000.0),
    }
}

#[test]
fn locate_places_a_point_cfi_by_its_spine_step() {
    let placed = index().locate("epubcfi(/6/4!/4/2/1:0)");
    assert_eq!(placed.spine_index, Some(1));
    assert_eq!(placed.chapter_title.as_deref(), Some("Two"));
    assert_eq!(placed.percent_through_book, Some(20.0));
}

#[test]
fn locate_names_the_chapter_a_shared_spine_document_opens_with() {
    // Two TOC entries in one spine document: only the first is defensible,
    // and every surface that places an anchor must agree on which.
    let mut index = index();
    index.chapters.push(EbookChapterRow {
        ordinal: 2,
        title: "Two-and-a-half".into(),
        href: "b.xhtml".into(),
        spine_index: 1,
        start_chars: 100,
    });
    assert_eq!(
        index
            .locate("epubcfi(/6/4!/4/2/1:0)")
            .chapter_title
            .as_deref(),
        Some("Two")
    );
}

#[test]
fn locate_places_a_range_cfi_the_same_way_a_highlight_carries_one() {
    let placed = index().locate("epubcfi(/6/6!/4,/2/1:0,/2/1:9)");
    assert_eq!(placed.spine_index, Some(2));
    // The last chapter starts at spine 1, so spine 2 still belongs to it.
    assert_eq!(placed.chapter_title.as_deref(), Some("Two"));
    assert_eq!(placed.percent_through_book, Some(80.0));
}

#[test]
fn locate_reads_a_bare_number_as_an_audiobook_timestamp() {
    let placed = index().locate("250");
    assert_eq!(placed.percent_through_book, Some(25.0));
    assert!(placed.spine_index.is_none());
}

#[test]
fn locate_places_nothing_for_an_anchor_it_cannot_read() {
    let placed = index().locate("kobo-span-nonsense");
    assert_eq!(placed, AnchorPlacement::default());
}

#[test]
fn locate_reports_the_spine_step_even_with_no_stats_to_measure_it_against() {
    let bare = AnchorIndex {
        spine: Vec::new(),
        chapters: Vec::new(),
        total_chars: 0,
        audio_seconds: None,
    };
    let placed = bare.locate("epubcfi(/6/4!/4/2/1:0)");
    assert_eq!(placed.spine_index, Some(1));
    assert!(placed.percent_through_book.is_none());
    assert!(placed.chapter_title.is_none());
}

#[test]
fn locate_ignores_a_timestamp_for_a_book_with_no_audio() {
    let no_audio = AnchorIndex {
        audio_seconds: None,
        ..index()
    };
    assert!(no_audio.locate("250").percent_through_book.is_none());
}

#[test]
fn annotation_order_parses_the_wire_tokens_and_defaults_to_position() {
    assert_eq!(
        AnnotationOrder::parse("chronological"),
        AnnotationOrder::Chronological
    );
    assert_eq!(
        AnnotationOrder::parse("position"),
        AnnotationOrder::Position
    );
    // An unrecognised value yields the useful order rather than an error.
    assert_eq!(
        AnnotationOrder::parse("sideways"),
        AnnotationOrder::Position
    );
    assert_eq!(AnnotationOrder::default(), AnnotationOrder::Position);
}

#[test]
fn position_key_sorts_unplaceable_anchors_last() {
    let placed = position_key(Some(3), 100, 1);
    let unplaced = position_key(None, 1, 2);
    assert!(
        placed < unplaced,
        "an unplaced anchor must not open the list"
    );
}
