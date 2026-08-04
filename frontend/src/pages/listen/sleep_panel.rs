//! Sleep-timer controls shared by the listen page and the mini-dock.
//!
//! [`SleepPanelBody`] renders the content — the preset grid (Off through
//! 4 hours), an "End of chapter" option, a live countdown status, and a
//! fade-out toggle; [`SleepPanel`] wraps it in the full player's scrim +
//! panel chrome, while the mini-dock hosts the same body inside its upward
//! popover. State lives in the app-scoped [`super::sleep::SleepController`].

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

use super::panel_shell::ListenPanelShell;
use super::sleep::{format_countdown, SleepChoice, PRESETS};

/// Reactive sleep-timer values + mutators shared by [`SleepPanel`] and
/// [`SleepPanelBody`], so the full-player wrapper can forward them 1:1
/// without duplicating the field list across both signatures.
#[derive(Clone, PartialEq)]
pub(super) struct SleepPanelState {
    pub remaining: Option<i32>,
    pub choice: SleepChoice,
    pub fade: bool,
    pub has_chapters: bool,
    pub on_select: EventHandler<i32>,
    pub on_end_of_chapter: EventHandler<()>,
    pub on_toggle_fade: EventHandler<()>,
}

/// Full-player sleep overlay: scrim + panel chrome around [`SleepPanelBody`].
#[component]
pub(super) fn SleepPanel(state: SleepPanelState, on_close: EventHandler<()>) -> Element {
    rsx! {
        ListenPanelShell {
            extra_class: "lp-sleep-panel",
            testid: "sleep-panel",
            on_close,
            SleepPanelBody { state }
        }
    }
}

/// Sleep-panel content — preset grid, end-of-chapter row, fade toggle — free
/// of positioning chrome so the full player and the mini-dock popover can
/// both host it.
#[component]
pub(super) fn SleepPanelBody(state: SleepPanelState) -> Element {
    let SleepPanelState {
        remaining,
        choice,
        fade,
        has_chapters,
        on_select,
        on_end_of_chapter,
        on_toggle_fade,
    } = state;
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
