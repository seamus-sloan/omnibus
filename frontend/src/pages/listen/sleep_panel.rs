//! Sleep-timer overlay for the listen page.
//!
//! Renders the preset grid (Off through 4 hours), an "End of chapter" option,
//! a live countdown status, and a fade-out toggle. State lives in the
//! [`super::sleep::SleepController`] owned by `ready_player`.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

use super::sleep::{format_countdown, SleepChoice, PRESETS};

#[component]
pub(super) fn SleepPanel(
    remaining: Option<i32>,
    choice: SleepChoice,
    fade: bool,
    has_chapters: bool,
    on_select: EventHandler<i32>,
    on_end_of_chapter: EventHandler<()>,
    on_toggle_fade: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let status = match remaining {
        Some(s) if s > 0 => format_countdown(s),
        _ => "\u{2014}".to_string(),
    };
    let status_label = if matches!(remaining, Some(s) if s > 0) {
        "remaining"
    } else {
        "inactive"
    };
    let eoc_on = matches!(choice, SleepChoice::EndOfChapter);
    let fade_cls = if fade {
        "lp-sleep-fade on"
    } else {
        "lp-sleep-fade"
    };

    rsx! {
        div {
            class: "lp-scrim",
            onclick: move |_| on_close.call(()),
        }

        div { class: "lp-panel lp-sleep-panel", "data-testid": "sleep-panel",
            div { class: "lp-panel-head",
                div {
                    div { class: "lp-panel-kicker", "Sleep timer" }
                    div { class: "lp-panel-title", "Drift off" }
                }
                div { class: "lp-sleep-status",
                    div { class: "lp-sleep-status-value", "data-testid": "sleep-status", "{status}" }
                    div { class: "label", "{status_label}" }
                }
            }

            div { class: "lp-sleep-grid",
                for (label , secs) in PRESETS.iter().copied() {
                    {
                        let active = match choice {
                            SleepChoice::Off => secs == 0,
                            SleepChoice::Preset(p) => p == secs && secs != 0,
                            SleepChoice::EndOfChapter => false,
                        };
                        let class = if active { "lp-sleep-btn on" } else { "lp-sleep-btn" };
                        rsx! {
                            button {
                                key: "{label}",
                                class: class,
                                r#type: "button",
                                onclick: move |_| on_select.call(secs),
                                "{label}"
                            }
                        }
                    }
                }
            }

            button {
                class: if eoc_on { "lp-sleep-eoc on" } else { "lp-sleep-eoc" },
                r#type: "button",
                disabled: !has_chapters,
                onclick: move |_| on_end_of_chapter.call(()),
                span { class: "lp-sleep-eoc-dot" }
                "End of chapter"
            }

            button {
                class: fade_cls,
                r#type: "button",
                "aria-pressed": fade,
                "aria-label": "Fade out volume",
                onclick: move |_| on_toggle_fade.call(()),
                span { class: "lp-sleep-fade-track",
                    span { class: "lp-sleep-fade-thumb" }
                }
                "Fade out volume"
            }
        }
    }
}
