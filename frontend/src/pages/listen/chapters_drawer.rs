//! Chapters drawer for the listen page.
//!
//! Right-side full-height panel showing the table of contents with
//! played / current / upcoming states. Clicking a row seeks to that
//! chapter's start position.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

use super::helpers::format_hms;

#[component]
pub(super) fn ChaptersDrawer(
    chapters: Vec<ChapterInfo>,
    current_chapter_index: usize,
    elapsed: f64,
    on_seek: EventHandler<f64>,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "lp-scrim",
            onclick: move |_| on_close.call(()),
        }

        div { class: "lp-drawer", "data-testid": "chapters-drawer",
            div { class: "lp-drawer-head",
                div {
                    div { class: "lp-panel-kicker", "Chapters" }
                    div { class: "lp-panel-title", "Contents" }
                }
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    "Close \u{2193}"
                }
            }
            div { class: "lp-drawer-body",
                if chapters.is_empty() {
                    div { class: "lp-drawer-empty",
                        p { class: "lp-drawer-empty-title", "No chapters" }
                        p { class: "lp-drawer-empty-detail",
                            "This audiobook has no embedded chapter markers."
                        }
                    }
                } else {
                    for (i, ch) in chapters.iter().enumerate() {
                        {
                            let is_played = i < current_chapter_index;
                            let is_current = i == current_chapter_index;
                            let row_class = if is_current {
                                "lp-drawer-row current"
                            } else if is_played {
                                "lp-drawer-row played"
                            } else {
                                "lp-drawer-row"
                            };
                            let row_testid = if is_current {
                                "chapter-row-current"
                            } else if is_played {
                                "chapter-row-played"
                            } else {
                                "chapter-row-upcoming"
                            };

                            let dur_label = format_hms(ch.duration_seconds);
                            let remaining_in_ch = if is_current {
                                let r = (ch.start_seconds + ch.duration_seconds - elapsed).max(0.0);
                                Some(format!("{} remaining", format_hms(r)))
                            } else {
                                None
                            };

                            let ch_start = ch.start_seconds;
                            let title = if ch.title.is_empty() {
                                format!("Chapter {}", i + 1)
                            } else {
                                ch.title.clone()
                            };
                            let ordinal = i + 1;

                            rsx! {
                                button {
                                    class: "{row_class}",
                                    "data-testid": "{row_testid}",
                                    r#type: "button",
                                    onclick: move |_| on_seek.call(ch_start),
                                    span { class: "lp-drawer-ord",
                                        if is_played {
                                            "\u{2713}"
                                        } else if is_current {
                                            "\u{25b6}"
                                        } else {
                                            "{ordinal}"
                                        }
                                    }
                                    span { class: "lp-drawer-title", "{title}" }
                                    span { class: "lp-drawer-dur",
                                        if let Some(rem) = remaining_in_ch {
                                            "{rem}"
                                        } else {
                                            "{dur_label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
