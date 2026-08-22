use omnibus_shared::{
    AlignmentLink, AlignmentMatch, AlignmentView, CrossFormatLinkMode, MappingConfidence,
};

use super::{
    align_mode, confirm_label, fmt_hm, foot_note, linear_pill, sub_header, AlignMode, SUB_DEFAULT,
};

fn bare_view(marks: i64) -> AlignmentView {
    AlignmentView {
        link: None,
        anchor_match: None,
        ebook: None,
        audio_files: Vec::new(),
        reading: None,
        listening: None,
        anchor_pairs: Vec::new(),
        audio_chapter_marks: marks,
    }
}

#[test]
fn align_mode_splits_stale_anchored_and_the_two_linear_modes() {
    let mut v = bare_view(0);
    assert_eq!(align_mode(&v), AlignMode::NoMarks);
    v.audio_chapter_marks = 23;
    assert_eq!(align_mode(&v), AlignMode::Mismatch);
    v.anchor_match = Some(AlignmentMatch {
        matched: 12,
        ebook_chapters: 14,
        confidence: MappingConfidence::ChapterAnchored,
    });
    assert_eq!(align_mode(&v), AlignMode::Anchored);
    // Stale wins over everything — the suppressed counts must not be
    // misread as a linear mode.
    v.link = Some(AlignmentLink {
        mode: CrossFormatLinkMode::Sequence,
        primary_book_file_id: None,
        stale: true,
        confirmed_at: 0,
        follow: false,
        user_anchors: 0,
    });
    assert_eq!(align_mode(&v), AlignMode::Stale);
}

#[test]
fn sub_header_explains_each_linear_mode_and_defaults_elsewhere() {
    assert_eq!(sub_header(AlignMode::Anchored, 23, Some(14)), SUB_DEFAULT);
    assert_eq!(sub_header(AlignMode::Stale, 0, None), SUB_DEFAULT);
    assert_eq!(
        sub_header(AlignMode::Mismatch, 23, Some(14)),
        "The audio carries 23 chapter marks but the book has 14 chapters, \
             so the chapters can\u{2019}t be paired up exactly."
    );
    assert_eq!(
        sub_header(AlignMode::Mismatch, 23, None),
        "The audio carries 23 chapter marks, but they can\u{2019}t be \
             paired up with the book\u{2019}s chapters exactly."
    );
    assert_eq!(
        sub_header(AlignMode::NoMarks, 0, None),
        "This audiobook carries no chapter markers, so there\u{2019}s \
             nothing to anchor the mapping to."
    );
}

#[test]
fn linear_pill_quotes_both_counts_and_degrades_without_the_ebook_side() {
    assert_eq!(
        linear_pill(23, Some(14)),
        "23 audio marks vs 14 chapters — no 1:1 match"
    );
    assert_eq!(linear_pill(23, None), "23 audio marks — no 1:1 match");
    assert_eq!(
        linear_pill(0, Some(14)),
        "no chapter marks in the audio — linear estimate"
    );
    // One of anything reads singular — "1 audio marks" shipped once.
    assert_eq!(
        linear_pill(1, Some(1)),
        "1 audio mark vs 1 chapter — no 1:1 match"
    );
    assert_eq!(
        sub_header(AlignMode::Mismatch, 1, None),
        "The audio carries 1 chapter mark, but it can\u{2019}t be \
             paired up with the book\u{2019}s chapters exactly."
    );
}

#[test]
fn confirm_label_names_the_mapping_mode_and_keeps_the_stale_wording() {
    assert_eq!(
        confirm_label(AlignMode::Anchored, false),
        "Sync Based On Chapters"
    );
    assert_eq!(
        confirm_label(AlignMode::Anchored, true),
        "Save order — Sync Based On Chapters"
    );
    assert_eq!(
        confirm_label(AlignMode::Mismatch, false),
        "Sync Based Off Percentage"
    );
    assert_eq!(
        confirm_label(AlignMode::NoMarks, true),
        "Save order — Sync Based Off Percentage"
    );
    assert_eq!(
        confirm_label(AlignMode::Stale, false),
        "Looks right — turn on sync"
    );
    assert_eq!(
        confirm_label(AlignMode::Stale, true),
        "Save order & turn on sync"
    );
}

#[test]
fn foot_note_is_omitted_for_the_percentage_modes_only() {
    assert!(foot_note(AlignMode::Anchored).is_some());
    assert!(foot_note(AlignMode::Stale).is_some());
    assert_eq!(foot_note(AlignMode::Mismatch), None);
    assert_eq!(foot_note(AlignMode::NoMarks), None);
}

#[test]
fn fmt_hm_floors_and_renders_hours_and_bare_minutes() {
    assert_eq!(fmt_hm(23_460.0), "6h 31m");
    assert_eq!(fmt_hm(2_460.0), "41m");
    assert_eq!(fmt_hm(0.0), "0m");
    assert_eq!(fmt_hm(-5.0), "0m");
    // 59:59 must not over-report as an hour.
    assert_eq!(fmt_hm(3_599.0), "59m");
}
