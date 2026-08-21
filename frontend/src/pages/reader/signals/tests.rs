use super::*;

// First-paint contract: `BookReadPage` seeds `status` from
// `ReaderStatus::default()` so SSR and the first WASM render produce
// identical markup (the `rd-overlay` loading node is present in both).
// Flipping this default would re-introduce the hydration mismatch
// described in .claude/rules/07-hydration.md.
#[test]
fn reader_status_default_is_loading_for_ssr_wasm_parity() {
    assert_eq!(ReaderStatus::default(), ReaderStatus::Loading);
}

#[test]
fn format_progress_labels_returns_empty_strings_before_first_relocate() {
    let (page, chapter) = format_progress_labels(&RelocateData::default());
    assert_eq!(page, "");
    assert_eq!(chapter, "");
}

#[test]
fn format_progress_labels_formats_page_and_chapter_strings() {
    let data = RelocateData {
        cfi: None,
        page: 42,
        total_pages: 300,
        pct: 14,
        frac: 0.14,
        pct_approx: false,
        at_end: false,
        chapter: 3,
        total_chapters: 24,
        chapter_title: String::new(),
        echo: false,
    };
    let (page, chapter) = format_progress_labels(&data);
    assert!(page.contains("p."));
    assert!(page.contains("42"));
    assert!(page.contains("300"));
    assert!(page.contains("14%"));
    assert!(chapter.contains("Ch"));
    assert!(chapter.contains("3"));
    assert!(chapter.contains("24"));
}

#[test]
fn format_ambient_page_returns_bare_page_number_when_paginated() {
    let data = RelocateData {
        page: 142,
        total_pages: 300,
        pct: 47,
        ..Default::default()
    };
    assert_eq!(format_ambient_page(&data), "142");
}

#[test]
fn format_ambient_page_falls_back_to_pct_then_empty() {
    let mut data = RelocateData {
        pct: 47,
        ..Default::default()
    };
    assert_eq!(format_ambient_page(&data), "47%");
    data.pct = 0;
    assert_eq!(format_ambient_page(&data), "");
}

#[test]
fn format_title_sub_formats_chapter_and_pct_and_falls_back() {
    let mut data = RelocateData {
        chapter: 14,
        pct: 68,
        ..Default::default()
    };
    assert_eq!(format_title_sub(&data), "Ch.\u{a0}14 \u{b7} 68%");
    data.chapter = 0;
    assert_eq!(format_title_sub(&data), "68%");
    assert_eq!(format_title_sub(&RelocateData::default()), "");
}

#[test]
fn format_contents_progress_formats_pages_and_is_empty_before_pagination() {
    let data = RelocateData {
        page: 184,
        total_pages: 272,
        pct: 68,
        ..Default::default()
    };
    assert_eq!(format_contents_progress(&data), "184 / 272 \u{b7} 68%");
    assert_eq!(format_contents_progress(&RelocateData::default()), "");
}

// Regression for issue #1234: a page turn must never render a stalled
// (unchanged) figure, even across a chapter boundary where `page` resets.
#[test]
fn format_progress_labels_resets_page_and_total_on_crossing_a_chapter_boundary() {
    let before = RelocateData {
        page: 22,
        total_pages: 22,
        pct: 40,
        chapter: 3,
        total_chapters: 24,
        ..Default::default()
    };
    let after = RelocateData {
        page: 1,
        total_pages: 8,
        pct: 41,
        chapter: 4,
        total_chapters: 24,
        ..Default::default()
    };
    let (before_page, _) = format_progress_labels(&before);
    let (after_page, _) = format_progress_labels(&after);
    assert_eq!(before_page, "p.\u{a0}22 of 22\u{a0}\u{b7}\u{a0}40%");
    assert_eq!(after_page, "p.\u{a0}1 of 8\u{a0}\u{b7}\u{a0}41%");
    assert_ne!(
        before_page, after_page,
        "a page turn must never render a stalled (unchanged) page figure"
    );
}

// Issue #1896 (AC2): while the locations map is still resolving, the
// percent renders as an explicit approximation ("~N%") rather than a
// frozen, falsely-precise figure.
#[test]
fn format_progress_labels_marks_an_approximate_pct_with_a_tilde() {
    let data = RelocateData {
        page: 1,
        total_pages: 20,
        pct: 47,
        pct_approx: true,
        ..Default::default()
    };
    let (page, _) = format_progress_labels(&data);
    assert_eq!(page, "p.\u{a0}1 of 20\u{a0}\u{b7}\u{a0}~47%");
}

#[test]
fn format_title_sub_and_ambient_page_and_contents_mark_approximate_pct() {
    let data = RelocateData {
        page: 3,
        total_pages: 20,
        pct: 47,
        pct_approx: true,
        chapter: 2,
        total_chapters: 50,
        ..Default::default()
    };
    assert_eq!(format_title_sub(&data), "Ch.\u{a0}2 \u{b7} ~47%");
    assert_eq!(format_contents_progress(&data), "3 / 20 \u{b7} ~47%");
    let bare = RelocateData {
        pct: 47,
        pct_approx: true,
        ..Default::default()
    };
    assert_eq!(format_ambient_page(&bare), "~47%");
}

// The glue's relocate payload flags the approximation as `pctApprox`;
// a payload without it (older glue, tests) must decode as exact.
#[test]
fn relocate_data_decodes_pct_approx_from_camel_case_and_defaults_false() {
    let with: RelocateData =
            serde_json::from_str(r#"{"page":1,"totalPages":2,"pct":40,"pctApprox":true,"chapter":0,"totalChapters":0,"chapterTitle":""}"#)
                .expect("decode");
    assert!(with.pct_approx);
    let without: RelocateData = serde_json::from_str(
        r#"{"page":1,"totalPages":2,"pct":40,"chapter":0,"totalChapters":0,"chapterTitle":""}"#,
    )
    .expect("decode");
    assert!(!without.pct_approx);
}

#[test]
fn format_progress_labels_falls_back_to_pct_only_when_total_pages_unknown() {
    let data = RelocateData {
        cfi: None,
        page: 0,
        total_pages: 0,
        pct: 7,
        frac: 0.07,
        pct_approx: false,
        at_end: false,
        chapter: 0,
        total_chapters: 0,
        chapter_title: String::new(),
        echo: false,
    };
    let (page, chapter) = format_progress_labels(&data);
    assert_eq!(page, "7%");
    assert_eq!(chapter, "");
}

fn toc_entry(label: &str) -> TocEntry {
    TocEntry {
        label: label.to_string(),
        href: format!("{label}.xhtml"),
        level: 0,
    }
}

fn relocate_with_title(title: &str) -> RelocateData {
    RelocateData {
        chapter_title: title.to_string(),
        ..Default::default()
    }
}

// Regression for issue #1909 (AC1): a front-matter spine item with no
// direct TOC entry must never read as chapter 0 — the previous chapter
// carries forward instead of the counter going backwards.
#[test]
fn resolve_chapter_position_carries_previous_chapter_forward_when_title_is_unmatched() {
    let toc = vec![toc_entry("Cover"), toc_entry("Chapter One")];
    let (chapter, total) = resolve_chapter_position(&toc, &relocate_with_title(""), 1);
    assert_eq!((chapter, total), (1, 2));
}

#[test]
fn resolve_chapter_position_matches_the_toc_array_position_when_the_title_resolves() {
    let toc = vec![
        toc_entry("Cover"),
        toc_entry("Dedication"),
        toc_entry("Chapter One"),
    ];
    let (chapter, total) = resolve_chapter_position(&toc, &relocate_with_title("Chapter One"), 1);
    assert_eq!((chapter, total), (3, 3));
}

#[test]
fn resolve_chapter_position_falls_back_to_incoming_values_before_the_toc_has_loaded() {
    let incoming = RelocateData {
        chapter: 4,
        total_chapters: 12,
        ..Default::default()
    };
    assert_eq!(resolve_chapter_position(&[], &incoming, 3), (4, 12));
}

// A matched title always wins outright, so a deliberate backward jump
// (TOC/bookmark navigation to an earlier chapter) is never clamped by
// the forward-carry rule above.
#[test]
fn resolve_chapter_position_allows_a_matched_title_to_move_backward() {
    let toc = vec![
        toc_entry("Cover"),
        toc_entry("Chapter One"),
        toc_entry("Chapter Two"),
    ];
    let (chapter, _) = resolve_chapter_position(&toc, &relocate_with_title("Chapter One"), 3);
    assert_eq!(chapter, 2);
}
