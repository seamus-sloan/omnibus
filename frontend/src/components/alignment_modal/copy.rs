//! The modal's wording: the mode it derives from the served view, the
//! sub-header and warn-pill sentences that explain a percentage mapping,
//! the confirm-button label, and the duration/recency formatters.

use omnibus_shared::AlignmentView;

/// `"6h 31m"` / `"41m"` for lane labels and the mapped preview. Floor
/// minutes — a duration label must never claim more time than exists.
pub(crate) fn fmt_hm(seconds: f64) -> String {
    let mins = (seconds.max(0.0) / 60.0).floor() as i64;
    let (h, m) = (mins / 60, mins % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// Coarse recency for sync copy ("yesterday"), from a client event clock.
/// These surfaces render post-mount only, so SSR never sees the value
/// (rule 07 holds) and `crate::time::now_unix` serves every target.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(crate) fn recency(clock_epoch_secs: i64) -> String {
    let d = (crate::time::now_unix() - clock_epoch_secs).max(0);
    if d < 90 {
        "just now".into()
    } else if d < 3_600 {
        format!("{}m ago", d / 60)
    } else if d < 86_400 {
        format!("{}h ago", d / 3_600)
    } else if d < 172_800 {
        "yesterday".into()
    } else {
        format!("{}d ago", d / 86_400)
    }
}

/// Which notice/confirm mode the modal is in, derived from the served
/// view. Stale wins — anchoring and the marks count are suppressed while
/// a link is stale, so their zeros must not be read as a linear mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum AlignMode {
    /// The audio set drifted since confirmation; re-confirm re-checks.
    Stale,
    /// Chapter anchoring engaged — jumps land chapter-accurately.
    Anchored,
    /// Marks exist but can't be paired 1:1 — percent mapping.
    Mismatch,
    /// No usable marks at all — percent mapping.
    NoMarks,
}

pub(super) fn align_mode(view: &AlignmentView) -> AlignMode {
    if view.link.as_ref().is_some_and(|l| l.stale) {
        AlignMode::Stale
    } else if view.anchor_match.is_some() {
        AlignMode::Anchored
    } else if view.audio_chapter_marks > 0 {
        AlignMode::Mismatch
    } else {
        AlignMode::NoMarks
    }
}

/// The ebook chapter count the mismatch copy quotes; `None` while the
/// structure backfill hasn't reached the book (no lane ticks either).
pub(super) fn ebook_chapter_count(view: &AlignmentView) -> Option<usize> {
    view.ebook
        .as_ref()
        .map(|e| e.chapters.len())
        .filter(|n| *n > 0)
}

/// The default judgement prompt under the title.
pub(super) const SUB_DEFAULT: &str =
    "Check that the mapped position lands about where it should, then confirm.";

/// Singular/plural noun for a count, so one mark never reads "1 marks".
fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The intro line under the title: the default prompt, or the honest
/// explanation of why the mapping is percentage-based.
pub(super) fn sub_header(mode: AlignMode, marks: i64, ebook_chapters: Option<usize>) -> String {
    let marks_n = plural(marks, "chapter mark", "chapter marks");
    match mode {
        AlignMode::Stale | AlignMode::Anchored => SUB_DEFAULT.into(),
        AlignMode::NoMarks => "This audiobook carries no chapter markers, so there\u{2019}s \
             nothing to anchor the mapping to."
            .into(),
        AlignMode::Mismatch => match ebook_chapters {
            Some(m) => {
                let chapters_n = plural(m as i64, "chapter", "chapters");
                format!(
                    "The audio carries {marks_n} but the book has {chapters_n}, \
                     so the chapters can\u{2019}t be paired up exactly."
                )
            }
            None => {
                let pronoun = if marks == 1 { "it" } else { "they" };
                format!(
                    "The audio carries {marks_n}, but {pronoun} can\u{2019}t be \
                     paired up with the book\u{2019}s chapters exactly."
                )
            }
        },
    }
}

/// The `~` warn pill's text for the two percentage modes. Without an
/// ebook chapter count the mismatch line drops the vs-clause rather than
/// quoting a number it doesn't have.
pub(super) fn linear_pill(marks: i64, ebook_chapters: Option<usize>) -> String {
    if marks == 0 {
        "no chapter marks in the audio — linear estimate".into()
    } else {
        let marks_n = plural(marks, "audio mark", "audio marks");
        match ebook_chapters {
            Some(m) => {
                let chapters_n = plural(m as i64, "chapter", "chapters");
                format!("{marks_n} vs {chapters_n} — no 1:1 match")
            }
            None => format!("{marks_n} — no 1:1 match"),
        }
    }
}

/// The fresh-confirm button names what confirming turns on; a pending
/// sequence reorder rides along as a prefix. The stale re-confirm keeps
/// its original labels — anchoring is suppressed there, so neither mode
/// name would be honest.
pub(super) fn confirm_label(mode: AlignMode, save_order: bool) -> &'static str {
    match (mode, save_order) {
        (AlignMode::Stale, false) => "Looks right — turn on sync",
        (AlignMode::Stale, true) => "Save order & turn on sync",
        (AlignMode::Anchored, false) => "Sync Based On Chapters",
        (AlignMode::Anchored, true) => "Save order — Sync Based On Chapters",
        (AlignMode::Mismatch | AlignMode::NoMarks, false) => "Sync Based Off Percentage",
        (AlignMode::Mismatch | AlignMode::NoMarks, true) => {
            "Save order — Sync Based Off Percentage"
        }
    }
}

/// The mono footer line; the percentage modes omit it per the design —
/// the warn banner already carries the caveat.
pub(super) fn foot_note(mode: AlignMode) -> Option<&'static str> {
    match mode {
        AlignMode::Stale | AlignMode::Anchored => {
            Some("Sync stays off until you confirm. You can unlink any time.")
        }
        AlignMode::Mismatch | AlignMode::NoMarks => None,
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::{
        AlignmentLink, AlignmentMatch, AlignmentView, CrossFormatLinkMode, MappingConfidence,
    };

    use super::{
        align_mode, confirm_label, fmt_hm, foot_note, linear_pill, sub_header, AlignMode,
        SUB_DEFAULT,
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
}
