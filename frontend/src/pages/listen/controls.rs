//! Transport controls, scrubber, toolbar, and top bar for the listen page.
//!
//! Pure presentation: receives playback-state values plus per-action handlers
//! from `ready_player`. The eval/interop layer lives in `bootstrap.rs`.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

use super::helpers::format_hms;

/// Top control bar styled to match the Omnibus brand bar. Renders the brand
/// wordmark on the left with a back button, and "Now playing" on the right.
#[component]
pub(super) fn TopBar(on_back: EventHandler<MouseEvent>) -> Element {
    rsx! {
        div { class: "lp-topbar",
            div { class: "lp-topbar-brand",
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    "data-testid": "listen-back",
                    "aria-label": "Back",
                    onclick: move |evt| on_back.call(evt),
                    "\u{2190} Back"
                }
            }
            div { style: "flex:1;" }
            span { class: "lp-kicker", "Now playing" }
        }
    }
}

/// The hidden HTML5 `<audio>` element bound by the JS shim.
#[component]
pub(super) fn AudioElement() -> Element {
    rsx! {
        audio {
            id: "omnibus-audio",
            "data-testid": "listen-audio",
            style: "display:none;",
            preload: "auto",
        }
    }
}

/// Scrub bar with custom-styled range input + elapsed / remaining / total.
#[component]
pub(super) fn Scrubber(
    elapsed: f64,
    duration: f64,
    remaining: f64,
    scrub_max: f64,
    fill_pct: f64,
    on_seek: EventHandler<Event<FormData>>,
) -> Element {
    let fill_style = format!("--fill: {fill_pct:.1}%");

    rsx! {
        div { class: "lp-scrubber",
            input {
                r#type: "range",
                class: "lp-scrub-input",
                min: "0",
                max: "{scrub_max}",
                step: "0.5",
                value: "{elapsed}",
                style: "{fill_style}",
                "aria-label": "Seek",
                "data-testid": "listen-scrub",
                oninput: move |evt| on_seek.call(evt),
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

/// Transport row: chapter-skip placeholders, ±30s seek, play/pause, rate.
#[component]
pub(super) fn TransportButtons(
    play_label: String,
    rate_label: String,
    rate_active: bool,
    on_toggle: EventHandler<MouseEvent>,
    on_skip_back: EventHandler<MouseEvent>,
    on_skip_forward: EventHandler<MouseEvent>,
    on_rate: EventHandler<MouseEvent>,
) -> Element {
    let rate_class = if rate_active {
        "lp-btn-rate on"
    } else {
        "lp-btn-rate"
    };

    rsx! {
        div { class: "lp-transport",
            // Previous chapter (disabled until chapter data ships)
            button {
                class: "lp-btn-ch",
                r#type: "button",
                disabled: true,
                "aria-label": "Previous chapter",
                title: "Previous chapter",
                span { class: "lp-ch-bar" }
                span { class: "lp-ch-tri-left" }
                span { "CH" }
            }

            button {
                class: "lp-btn-skip",
                r#type: "button",
                "data-testid": "listen-skip-back",
                "aria-label": "Back 30 seconds",
                onclick: move |evt| on_skip_back.call(evt),
                "-30"
            }

            button {
                class: "lp-btn-play",
                r#type: "button",
                "data-testid": "listen-toggle",
                "aria-label": "{play_label}",
                onclick: move |evt| on_toggle.call(evt),
                "{play_label}"
            }

            button {
                class: "lp-btn-skip",
                r#type: "button",
                "data-testid": "listen-skip-forward",
                "aria-label": "Forward 30 seconds",
                onclick: move |evt| on_skip_forward.call(evt),
                "+30"
            }

            // Next chapter (disabled until chapter data ships)
            button {
                class: "lp-btn-ch",
                r#type: "button",
                disabled: true,
                "aria-label": "Next chapter",
                title: "Next chapter",
                span { "CH" }
                span { class: "lp-ch-tri-right" }
                span { class: "lp-ch-bar" }
            }

            button {
                class: rate_class,
                r#type: "button",
                "data-testid": "listen-rate",
                "aria-label": "Playback speed",
                onclick: move |evt| on_rate.call(evt),
                "{rate_label}"
            }
        }
    }
}

/// Toolbar row beneath transport: Sleep, Bookmark, Chapters.
/// All handlers are wired by `ready_player`; the buttons themselves are inert
/// placeholders until their backing PRs land.
#[component]
pub(super) fn Toolbar(
    on_speed: EventHandler<MouseEvent>,
    on_sleep: EventHandler<MouseEvent>,
    on_bookmark: EventHandler<MouseEvent>,
    on_chapters: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "lp-toolbar",
            button {
                class: "btn sm",
                r#type: "button",
                onclick: move |evt| on_sleep.call(evt),
                disabled: true,
                "Sleep \u{00b7} off"
            }
            button {
                class: "btn sm",
                r#type: "button",
                onclick: move |evt| on_bookmark.call(evt),
                disabled: true,
                "Bookmark"
            }
            button {
                class: "btn sm",
                r#type: "button",
                onclick: move |evt| on_chapters.call(evt),
                disabled: true,
                "Chapters"
            }
        }
    }
}
