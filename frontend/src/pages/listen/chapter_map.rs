//! Segmented chapter progress bar replacing the old range-input scrubber.
//!
//! Each chapter is a flex segment sized by `flex: {duration_seconds}`. The
//! played portion within each segment fills proportionally with accent colour.
//! Clicking a segment seeks to its start time.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

use super::helpers::format_hms;

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
        let fill_pct = if duration > 0.0 {
            (elapsed / duration * 100.0).min(100.0)
        } else {
            0.0
        };
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

                        let fill_pct = if i < current_chapter_index {
                            100.0
                        } else if is_current && ch.duration_seconds > 0.0 {
                            ((elapsed - ch.start_seconds) / ch.duration_seconds * 100.0)
                                .clamp(0.0, 100.0)
                        } else {
                            0.0
                        };

                        let class_name = if is_current {
                            "lp-chapter-seg current"
                        } else {
                            "lp-chapter-seg"
                        };

                        let ch_start = ch.start_seconds;
                        let title = ch.title.clone();

                        rsx! {
                            div {
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
