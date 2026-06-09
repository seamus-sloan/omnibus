//! Sleep-timer overlay for the listen page.
//!
//! Shows a preset grid of durations (Off through 4 hours) plus an
//! "End of chapter" option. Currently visual-only — the actual
//! countdown timer ships in PR 5.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

const PRESETS: &[&str] = &[
    "Off", "15 min", "30 min", "45 min", "1 hour", "2 hours", "3 hours", "4 hours",
];

// Pure presentational shell — no branching logic to unit-test.
// Active/inactive state is owned by `ready_player` (already tested).
// Covered by Playwright at ui_tests/playwright/tests/flows/.

/// Frosted-glass sleep-timer panel with duration presets.
#[component]
pub(super) fn SleepPanel(on_close: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "lp-scrim",
            onclick: move |_| on_close.call(()),
        }

        div { class: "lp-panel lp-sleep-panel",
            div { class: "lp-panel-head",
                div {
                    div { class: "lp-panel-kicker", "Sleep timer" }
                    div { class: "lp-panel-title", "Drift off" }
                }
                div { class: "lp-sleep-status",
                    div { class: "lp-sleep-status-value", "\u{2014}" }
                    div { class: "label", "inactive" }
                }
            }

            div { class: "lp-sleep-grid",
                for preset in PRESETS.iter() {
                    {
                        let is_off = *preset == "Off";
                        let class = if is_off { "lp-sleep-btn on" } else { "lp-sleep-btn" };
                        rsx! {
                            button {
                                key: "{preset}",
                                class: class,
                                r#type: "button",
                                disabled: !is_off,
                                "{preset}"
                            }
                        }
                    }
                }
            }

            button {
                class: "lp-sleep-eoc",
                r#type: "button",
                disabled: true,
                span { class: "lp-sleep-eoc-dot" }
                "End of chapter"
            }
        }
    }
}
