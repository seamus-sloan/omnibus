//! The alignment status readout: mutually exclusive with priority order —
//! a stale link pauses everything else, then a chapter-anchored match,
//! then the percent-mapping warning with its two-way copy.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentMatch, AlignmentView};

use super::copy::{ebook_chapter_count, linear_pill};

/// Which of the three status readouts applies to a served view. `Stale`
/// wins over an anchor match, matching `copy::align_mode`'s branch order
/// — kept as a separate check here rather than sharing that enum because
/// this readout also carries the `AlignmentMatch` payload the anchored
/// branch renders.
#[derive(Debug, Clone, Copy, PartialEq)]
enum NoticeKind {
    Stale,
    Anchored(AlignmentMatch),
    Percentage,
}

fn notice_kind(view: &AlignmentView) -> NoticeKind {
    if view.link.as_ref().is_some_and(|l| l.stale) {
        NoticeKind::Stale
    } else if let Some(m) = view.anchor_match {
        NoticeKind::Anchored(m)
    } else {
        NoticeKind::Percentage
    }
}

/// Renders whichever of the three status readouts applies to the served
/// view. `Stale` wins over an anchor match — see `copy::align_mode`.
pub(super) fn render_notice(view: &AlignmentView) -> Element {
    let kind = notice_kind(view);
    rsx! {
        if matches!(kind, NoticeKind::Stale) {
            // Anchoring (and the marks count) is suppressed while stale —
            // say so instead of misreading the zeros.
            p { class: "al-lowconf", role: "note", "data-testid": "alignment-lowconf",
                "The audio files changed since you linked — sync is paused, and "
                "the alignment re-checks when you confirm again."
            }
        } else if let NoticeKind::Anchored(m) = kind {
            p { class: "al-matched", role: "note", "data-testid": "alignment-match",
                "\u{2713} {m.matched} of {m.ebook_chapters} chapters matched — "
                "jumps land chapter-accurately."
            }
        } else {
            p { class: "al-matched al-matched-warn", role: "note", "data-testid": "alignment-linear-pill",
                "~ {linear_pill(view.audio_chapter_marks, ebook_chapter_count(view))}"
            }
            div { class: "al-lowconf-callout", role: "note", "data-testid": "alignment-lowconf",
                span { class: "al-warn-badge", "!" }
                if view.audio_chapter_marks > 0 {
                    div {
                        strong { "The chapter counts don\u{2019}t match, so sync will go by percentage instead. " }
                        "Jumps will land close, but not exactly. Extra marks are usually "
                        "opening notes, bonus scenes, credits, remarks, or part numbers."
                    }
                } else {
                    div {
                        strong { "This mapping is a straight percent-for-percent estimate. " }
                        "Jumps will land close, not exact — expect drift of several "
                        "minutes, especially mid-book. Adding chapter marks to the audio "
                        "file later will tighten it up."
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::{
        AlignmentLink, AlignmentMatch, AlignmentView, CrossFormatLinkMode, MappingConfidence,
    };

    use super::{notice_kind, NoticeKind};

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
    fn notice_kind_prefers_percentage_when_nothing_else_applies() {
        assert_eq!(notice_kind(&bare_view(0)), NoticeKind::Percentage);
        assert_eq!(notice_kind(&bare_view(23)), NoticeKind::Percentage);
    }

    #[test]
    fn notice_kind_reports_anchored_with_the_match_payload() {
        let mut v = bare_view(23);
        let m = AlignmentMatch {
            matched: 12,
            ebook_chapters: 14,
            confidence: MappingConfidence::ChapterAnchored,
        };
        v.anchor_match = Some(m);
        assert_eq!(notice_kind(&v), NoticeKind::Anchored(m));
    }

    #[test]
    fn notice_kind_stale_wins_over_an_anchor_match() {
        let mut v = bare_view(23);
        v.anchor_match = Some(AlignmentMatch {
            matched: 12,
            ebook_chapters: 14,
            confidence: MappingConfidence::ChapterAnchored,
        });
        // Suppressed counts on a stale link must not be misread as the
        // anchored notice — same precedence copy::align_mode documents.
        v.link = Some(AlignmentLink {
            mode: CrossFormatLinkMode::Sequence,
            primary_book_file_id: None,
            stale: true,
            confirmed_at: 0,
            follow: false,
            user_anchors: 0,
        });
        assert_eq!(notice_kind(&v), NoticeKind::Stale);
    }

    #[test]
    fn notice_kind_is_percentage_when_link_is_present_but_not_stale() {
        let mut v = bare_view(0);
        v.link = Some(AlignmentLink {
            mode: CrossFormatLinkMode::Sequence,
            primary_book_file_id: None,
            stale: false,
            confirmed_at: 0,
            follow: false,
            user_anchors: 0,
        });
        assert_eq!(notice_kind(&v), NoticeKind::Percentage);
    }
}
