//! Transport controls, scrubber, toolbar, and top bar for the listen page.
//!
//! Pure presentation: receives playback-state values plus per-action handlers
//! from `ready_player`. The eval/interop layer lives in `bootstrap.rs`.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

use super::helpers::format_hms;

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

/// Transport row: chapter-skip, ±30s seek, play/pause, rate.
#[component]
pub(super) fn TransportButtons(
    play_label: String,
    playing: bool,
    rate_label: String,
    rate_active: bool,
    has_chapters: bool,
    on_toggle: EventHandler<MouseEvent>,
    on_skip_back: EventHandler<MouseEvent>,
    on_skip_forward: EventHandler<MouseEvent>,
    on_rate: EventHandler<MouseEvent>,
    on_chapter_prev: EventHandler<MouseEvent>,
    on_chapter_next: EventHandler<MouseEvent>,
) -> Element {
    let rate_class = if rate_active {
        "lp-btn-rate on"
    } else {
        "lp-btn-rate"
    };

    rsx! {
        div { class: "lp-transport",
            button {
                class: "lp-btn-ch",
                r#type: "button",
                disabled: !has_chapters,
                "aria-label": "Previous chapter",
                title: "Previous chapter",
                onclick: move |evt| on_chapter_prev.call(evt),
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
                if playing {
                    div { class: "lp-ico-pause",
                        div { class: "lp-ico-pause-bar" }
                        div { class: "lp-ico-pause-bar" }
                    }
                } else {
                    div { class: "lp-ico-play" }
                }
            }

            button {
                class: "lp-btn-skip",
                r#type: "button",
                "data-testid": "listen-skip-forward",
                "aria-label": "Forward 30 seconds",
                onclick: move |evt| on_skip_forward.call(evt),
                "+30"
            }

            button {
                class: "lp-btn-ch",
                r#type: "button",
                disabled: !has_chapters,
                "aria-label": "Next chapter",
                title: "Next chapter",
                onclick: move |evt| on_chapter_next.call(evt),
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
/// Each button toggles its overlay panel; `*_active` props drive the
/// highlighted state. The panels themselves are visual shells until
/// their backing infrastructure ships (PRs 3–5).
#[component]
pub(super) fn Toolbar(
    sleep_active: bool,
    bookmarks_active: bool,
    chapters_active: bool,
    on_sleep: EventHandler<MouseEvent>,
    on_bookmark: EventHandler<MouseEvent>,
    on_chapters: EventHandler<MouseEvent>,
) -> Element {
    let sleep_class = if sleep_active {
        "btn sm lp-toolbar-btn on"
    } else {
        "btn sm lp-toolbar-btn"
    };
    let bm_class = if bookmarks_active {
        "btn sm lp-toolbar-btn on"
    } else {
        "btn sm lp-toolbar-btn"
    };
    let ch_class = if chapters_active {
        "btn sm lp-toolbar-btn on"
    } else {
        "btn sm lp-toolbar-btn"
    };
    let sleep_label = if sleep_active {
        "Sleep \u{00b7} on"
    } else {
        "Sleep \u{00b7} off"
    };
    let ch_label = if chapters_active {
        "Chapters \u{2193}"
    } else {
        "Chapters \u{2191}"
    };

    rsx! {
        div { class: "lp-toolbar",
            button {
                class: sleep_class,
                r#type: "button",
                onclick: move |evt| on_sleep.call(evt),
                "{sleep_label}"
            }
            button {
                class: bm_class,
                r#type: "button",
                onclick: move |evt| on_bookmark.call(evt),
                "Bookmark"
            }
            button {
                class: ch_class,
                r#type: "button",
                onclick: move |evt| on_chapters.call(evt),
                "{ch_label}"
            }
        }
    }
}
