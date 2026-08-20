//! The alignment status readout: mutually exclusive with priority order —
//! a stale link pauses everything else, then a chapter-anchored match,
//! then the percent-mapping warning with its two-way copy.

use dioxus::prelude::*;
use omnibus_shared::AlignmentView;

use super::copy::{ebook_chapter_count, linear_pill};

/// Renders whichever of the three status readouts applies to the served
/// view. `Stale` wins over an anchor match — see `copy::align_mode`.
pub(super) fn render_notice(view: &AlignmentView) -> Element {
    rsx! {
        if view.link.as_ref().is_some_and(|l| l.stale) {
            // Anchoring (and the marks count) is suppressed while stale —
            // say so instead of misreading the zeros.
            p { class: "al-lowconf", role: "note", "data-testid": "alignment-lowconf",
                "The audio files changed since you linked — sync is paused, and "
                "the alignment re-checks when you confirm again."
            }
        } else if let Some(m) = view.anchor_match {
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
