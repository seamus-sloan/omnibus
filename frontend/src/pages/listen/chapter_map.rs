//! Segmented chapter progress bar replacing the old range-input scrubber.
//!
//! Each chapter is a flex segment sized by `flex: {duration_seconds}`. The
//! played portion within each segment fills proportionally with accent colour.
//! Clicking a segment seeks to its start time.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

use super::helpers::format_hms;

/// Fill percentage for a chapter segment in the progress bar.
/// `i` is the segment index, `current` is the active chapter index.
/// Returns 100 for fully played chapters, 0 for upcoming ones, and a
/// clamped proportional value for the active chapter.
pub(super) fn chapter_fill_pct(
    i: usize,
    current: usize,
    elapsed: f64,
    ch_start: f64,
    ch_duration: f64,
) -> f64 {
    if i < current {
        100.0
    } else if i == current && ch_duration > 0.0 {
        ((elapsed - ch_start) / ch_duration * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Fill percentage when there are no chapter markers — falls back to a
/// single-segment proportional bar based on total duration.
pub(super) fn no_chapter_fill_pct(elapsed: f64, duration: f64) -> f64 {
    if duration > 0.0 {
        (elapsed / duration * 100.0).min(100.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_fill_pct_returns_100_for_past_chapters() {
        assert!((chapter_fill_pct(0, 2, 500.0, 0.0, 200.0) - 100.0).abs() < f64::EPSILON);
        assert!((chapter_fill_pct(1, 2, 500.0, 200.0, 200.0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn chapter_fill_pct_returns_0_for_upcoming_chapters() {
        assert!((chapter_fill_pct(3, 1, 300.0, 600.0, 200.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn chapter_fill_pct_returns_proportional_for_current_chapter() {
        // 50 s into a 200 s chapter → 25%
        let pct = chapter_fill_pct(1, 1, 250.0, 200.0, 200.0);
        assert!((pct - 25.0).abs() < 0.001);
    }

    #[test]
    fn chapter_fill_pct_clamps_current_chapter_at_100() {
        // elapsed past end of chapter should not exceed 100%
        let pct = chapter_fill_pct(0, 0, 999.0, 0.0, 200.0);
        assert!((pct - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_chapter_fill_pct_is_proportional_to_duration() {
        let pct = no_chapter_fill_pct(30.0, 120.0);
        assert!((pct - 25.0).abs() < 0.001);
    }

    #[test]
    fn no_chapter_fill_pct_returns_zero_when_duration_is_zero() {
        assert!((no_chapter_fill_pct(0.0, 0.0)).abs() < f64::EPSILON);
    }
}

/// The chapter progress map component.
#[component]
pub(super) fn ChapterMap(
    chapters: Vec<ChapterInfo>,
    elapsed: f64,
    duration: f64,
    remaining: f64,
    current_chapter_index: usize,
    on_seek: EventHandler<f64>,
) -> Element {
    if chapters.is_empty() {
        let fill_pct = no_chapter_fill_pct(elapsed, duration);
        return rsx! {
            div { class: "lp-chapter-map",
                div { class: "lp-chapter-seg-row",
                    div {
                        class: "lp-chapter-seg",
                        style: "flex: 1;",
                        div {
                            class: "lp-chapter-seg-fill",
                            style: "width: {fill_pct:.1}%;",
                        }
                    }
                }
                div { class: "lp-scrub-times",
                    span { "{format_hms(elapsed)}" }
                    span { class: "lp-scrub-remaining",
                        "\u{00b7} {format_hms(remaining)} remaining"
                    }
                    span { "{format_hms(duration)}" }
                }
            }
        };
    }

    rsx! {
        div { class: "lp-chapter-map",
            div { class: "lp-chapter-seg-row", "data-testid": "chapter-map",
                for (i, ch) in chapters.iter().enumerate() {
                    {
                        let flex_val = ch.duration_seconds.max(0.1);
                        let is_current = i == current_chapter_index;

                        let fill_pct = chapter_fill_pct(
                            i,
                            current_chapter_index,
                            elapsed,
                            ch.start_seconds,
                            ch.duration_seconds,
                        );

                        let class_name = if is_current {
                            "lp-chapter-seg current"
                        } else {
                            "lp-chapter-seg"
                        };

                        let ch_start = ch.start_seconds;
                        let title = ch.title.clone();

                        rsx! {
                            div {
                                key: "{ch.ordinal}",
                                class: "{class_name}",
                                style: "flex: {flex_val};",
                                title: "{title}",
                                onclick: move |_| on_seek.call(ch_start),
                                div {
                                    class: "lp-chapter-seg-fill",
                                    style: "width: {fill_pct:.1}%;",
                                }
                            }
                        }
                    }
                }
            }
            div { class: "lp-scrub-times",
                span { "{format_hms(elapsed)}" }
                span { class: "lp-scrub-remaining",
                    "\u{00b7} {format_hms(remaining)} remaining"
                }
                span { "{format_hms(duration)}" }
            }
        }
    }
}
