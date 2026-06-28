//! Transport controls, toolbar, and top bar for the listen page.
//!
//! Pure presentation: receives playback-state values plus per-action handlers
//! from `ready_player`. The eval/interop layer lives in `bootstrap.rs`.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

// --- Pure helpers extracted so they can be unit-tested without a renderer.

/// CSS class for the rate button — appends `" on"` when the speed panel is open.
pub(super) fn rate_btn_class(active: bool) -> &'static str {
    if active {
        "lp-btn-rate on"
    } else {
        "lp-btn-rate"
    }
}

/// CSS class for any toolbar toggle button — appends `" on"` when the panel is open.
pub(super) fn toolbar_btn_class(active: bool) -> &'static str {
    if active {
        "btn sm lp-toolbar-btn on"
    } else {
        "btn sm lp-toolbar-btn"
    }
}

/// Label for the sleep toolbar button, reflecting active state.
pub(super) fn sleep_label(active: bool) -> &'static str {
    if active {
        "Sleep \u{00b7} on"
    } else {
        "Sleep \u{00b7} off"
    }
}

/// Label for the chapters toolbar button, reflecting open/closed state.
pub(super) fn chapters_toggle_label(open: bool) -> &'static str {
    if open {
        "Chapters \u{2193}"
    } else {
        "Chapters \u{2191}"
    }
}

/// The hidden HTML5 `<audio>` element bound by the JS shim. Mounted once at
/// the App root (not per-route) so playback persists across navigation.
#[component]
pub(crate) fn AudioElement() -> Element {
    rsx! {
        audio {
            id: "omnibus-audio",
            "data-testid": "listen-audio",
            style: "display:none;",
            preload: "auto",
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
    let rate_class = rate_btn_class(rate_active);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_btn_class_inactive_returns_base_class() {
        assert_eq!(rate_btn_class(false), "lp-btn-rate");
    }

    #[test]
    fn rate_btn_class_active_appends_on() {
        assert_eq!(rate_btn_class(true), "lp-btn-rate on");
    }

    #[test]
    fn toolbar_btn_class_inactive_returns_base_class() {
        assert_eq!(toolbar_btn_class(false), "btn sm lp-toolbar-btn");
    }

    #[test]
    fn toolbar_btn_class_active_appends_on() {
        assert_eq!(toolbar_btn_class(true), "btn sm lp-toolbar-btn on");
    }

    #[test]
    fn sleep_label_inactive_shows_off() {
        assert_eq!(sleep_label(false), "Sleep \u{00b7} off");
    }

    #[test]
    fn sleep_label_active_shows_on() {
        assert_eq!(sleep_label(true), "Sleep \u{00b7} on");
    }

    #[test]
    fn chapters_toggle_label_closed_shows_up_arrow() {
        assert_eq!(chapters_toggle_label(false), "Chapters \u{2191}");
    }

    #[test]
    fn chapters_toggle_label_open_shows_down_arrow() {
        assert_eq!(chapters_toggle_label(true), "Chapters \u{2193}");
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
    let sleep_cls = toolbar_btn_class(sleep_active);
    let bm_class = toolbar_btn_class(bookmarks_active);
    let ch_class = toolbar_btn_class(chapters_active);
    let slp_label = sleep_label(sleep_active);
    let ch_label = chapters_toggle_label(chapters_active);

    rsx! {
        div { class: "lp-toolbar",
            button {
                class: sleep_cls,
                r#type: "button",
                onclick: move |evt| on_sleep.call(evt),
                "{slp_label}"
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
