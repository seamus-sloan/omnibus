//! Frosted-glass speed controls shared by the listen page and the mini-dock.
//!
//! [`SpeedPanelBody`] renders the content — a preset grid (0.5–2.0×), a
//! fine-tune slider (0.5–3.0×), and a ±0.05 stepper; [`SpeedPanel`] wraps it
//! in the full player's scrim + panel chrome, while the mini-dock hosts the
//! same body inside its upward popover. Rate changes are persisted per-book
//! via [`crate::audiobook_progress`] and forwarded to the JS audio shim.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{
    MAX_AUDIOBOOK_PLAYBACK_RATE as MAX_RATE, MIN_AUDIOBOOK_PLAYBACK_RATE as MIN_RATE,
};

use super::helpers::{apply_rate, RATE_STEP as STEP};
use super::panel_shell::ListenPanelShell;

const PRESETS: &[f64] = &[0.5, 0.8, 1.0, 1.1, 1.2, 1.5, 1.8, 2.0];

/// Full-player speed overlay: scrim + panel chrome around [`SpeedPanelBody`].
#[component]
pub(super) fn SpeedPanel(
    rate: Signal<f64>,
    rate_error: Signal<Option<String>>,
    user_id: Option<i64>,
    uuid: String,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        ListenPanelShell {
            extra_class: "lp-speed-panel",
            on_close,
            SpeedPanelBody { rate, rate_error, user_id, uuid }
        }
    }
}

/// Speed-panel content — preset grid, fine-tune slider, stepper — free of
/// positioning chrome so the full player and the mini-dock popover can both
/// host it.
#[component]
pub(super) fn SpeedPanelBody(
    rate: Signal<f64>,
    rate_error: Signal<Option<String>>,
    user_id: Option<i64>,
    uuid: String,
) -> Element {
    let cur = rate();
    let rate_label = format!("{cur:.1}\u{00d7}");
    let rate_precise = format!("{cur:.2}\u{00d7}");
    let slider_pct = ((cur - MIN_RATE) / (MAX_RATE - MIN_RATE) * 100.0).clamp(0.0, 100.0);
    let fill_width = format!("{slider_pct:.1}%");
    let thumb_left = format!("calc({slider_pct:.1}% - 9px)");

    rsx! {
            div { class: "lp-panel-head",
                div {
                    div { class: "lp-panel-kicker", "Playback speed" }
                    div { class: "lp-panel-title", "Set your pace" }
                }
                div { class: "lp-speed-value", "{rate_label}" }
            }

            div { class: "lp-speed-grid",
                for val in PRESETS.iter().copied() {
                    {
                        let uuid = uuid.clone();
                        let is_on = (val - cur).abs() < STEP / 2.0;
                        let class = if is_on { "lp-speed-btn on" } else { "lp-speed-btn" };
                        rsx! {
                            button {
                                key: "{val}",
                                class: class,
                                r#type: "button",
                                onclick: move |_| {
                                    apply_rate(&mut rate, rate_error, user_id, &uuid, val);
                                },
                                "{val:.1}\u{00d7}"
                            }
                        }
                    }
                }
            }

            div { class: "lp-speed-fine",
                div { class: "lp-speed-fine-header",
                    span { class: "label", "Fine-tune" }
                    span { class: "label", "0.5\u{00d7} \u{2014} 3.0\u{00d7}" }
                }

                div { class: "lp-speed-fine-track",
                    div { class: "lp-speed-fine-ticks",
                        for i in 0..11 {
                            {
                                let h = if i % 5 == 0 { 12 } else { 7 };
                                rsx! {
                                    div {
                                        key: "{i}",
                                        class: "lp-speed-fine-tick",
                                        style: "height: {h}px;",
                                    }
                                }
                            }
                        }
                    }
                    div { class: "lp-speed-fine-rail" }
                    div { class: "lp-speed-fine-fill", style: "width: {fill_width};" }
                    div { class: "lp-speed-fine-thumb", style: "left: {thumb_left};" }

                    {
                        let uuid = uuid.clone();
                        rsx! {
                            input {
                                r#type: "range",
                                class: "lp-scrub-input",
                                min: "{MIN_RATE}",
                                max: "{MAX_RATE}",
                                step: "{STEP}",
                                value: "{cur}",
                                "aria-label": "Fine-tune speed",
                                style: "position:absolute; inset:0; opacity:0; cursor:pointer; width:100%; height:100%;",
                                oninput: move |evt: Event<FormData>| {
                                    if let Ok(v) = evt.value().parse::<f64>() {
                                        apply_rate(&mut rate, rate_error, user_id, &uuid, v);
                                    }
                                },
                            }
                        }
                    }
                }
            }

            div { class: "lp-speed-stepper",
                {
                    let uuid = uuid.clone();
                    rsx! {
                        button {
                            class: "lp-speed-step-btn",
                            r#type: "button",
                            "aria-label": "Decrease speed",
                            onclick: move |_| {
                                apply_rate(&mut rate, rate_error, user_id, &uuid, cur - STEP);
                            },
                            "\u{2212}"
                        }
                    }
                }
                div { class: "lp-speed-step-value",
                    div { class: "mono", style: "font-size:16px; color:var(--ink-0);",
                        "{rate_precise}"
                    }
                    div { class: "label", style: "margin-top:2px;",
                        "0.05 steps"
                    }
                }
                {
                    let uuid = uuid.clone();
                    rsx! {
                        button {
                            class: "lp-speed-step-btn",
                            r#type: "button",
                            "aria-label": "Increase speed",
                            onclick: move |_| {
                                apply_rate(&mut rate, rate_error, user_id, &uuid, cur + STEP);
                            },
                            "+"
                        }
                    }
                }
            }
    }
}
