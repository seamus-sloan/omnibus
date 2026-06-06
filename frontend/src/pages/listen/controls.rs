//! Transport controls + scrubber for the listen page.
//!
//! Split from `BookListenPage` to keep each component body under ~150 lines.
//! Receives the playback-state signals plus per-action [`EventHandler`]s
//! from the parent — the eval/interop layer stays in `bootstrap.rs` /
//! the parent orchestrator so this module is pure presentation.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

use super::helpers::format_hms;

/// Slim top control bar: back button on the left, "Now playing" label on
/// the right. Receives the back-action handler from the parent so route
/// navigation logic stays in the orchestrator.
#[component]
pub(super) fn TopBar(on_back: EventHandler<MouseEvent>) -> Element {
    rsx! {
        div {
            class: "player-bar",
            style: "display:flex; align-items:center; gap:0.5rem; padding:0.5rem 0.75rem; border-bottom:1px solid var(--line);",
            button {
                class: "btn ghost sm",
                r#type: "button",
                "data-testid": "listen-back",
                "aria-label": "Back",
                onclick: move |evt| on_back.call(evt),
                "\u{2190} Back"
            }
            div { style: "flex:1;" }
            span {
                class: "label",
                style: "color:var(--ink-2); font-family:var(--mono); font-size:12px;",
                "Now playing"
            }
        }
    }
}

/// The hidden HTML5 `<audio>` element bound by the JS shim. Kept as a
/// component so the orchestrator doesn't have to inline the attribute
/// block — purely structural, no inputs.
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

/// Scrub bar + elapsed / remaining / total timestamps.
#[component]
pub(super) fn Scrubber(
    elapsed: f64,
    duration: f64,
    remaining: f64,
    scrub_max: f64,
    on_seek: EventHandler<Event<FormData>>,
) -> Element {
    rsx! {
        div { style: "margin-top:32px;",
            input {
                r#type: "range",
                min: "0",
                max: "{scrub_max}",
                step: "0.5",
                value: "{elapsed}",
                "aria-label": "Seek",
                "data-testid": "listen-scrub",
                style: "width:100%;",
                oninput: move |evt| on_seek.call(evt),
            }
            div {
                style: "display:flex; justify-content:space-between; margin-top:8px; font-family:var(--mono); font-size:12px; color:var(--ink-2);",
                span { "{format_hms(elapsed)}" }
                span {
                    style: "color:var(--ink-3);",
                    "\u{00b7} {format_hms(remaining)} remaining"
                }
                span { "{format_hms(duration)}" }
            }
        }
    }
}

/// Skip-back / play-pause / skip-forward / rate cycle row.
#[component]
pub(super) fn TransportButtons(
    play_label: String,
    rate_label: String,
    on_toggle: EventHandler<MouseEvent>,
    on_skip_back: EventHandler<MouseEvent>,
    on_skip_forward: EventHandler<MouseEvent>,
    on_rate: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div {
            style: "display:flex; align-items:center; justify-content:center; gap:18px; margin-top:24px;",
            button {
                class: "btn ghost",
                style: "width:48px; height:48px; padding:0; border-radius:999px; font-family:var(--mono); font-size:11px;",
                r#type: "button",
                "data-testid": "listen-skip-back",
                "aria-label": "Back 30 seconds",
                onclick: move |evt| on_skip_back.call(evt),
                "-30"
            }
            button {
                style: "width:72px; height:72px; border-radius:999px; background:var(--accent); color:var(--accent-ink); border:0; cursor:pointer; font-size:16px; font-weight:600;",
                r#type: "button",
                "data-testid": "listen-toggle",
                "aria-label": "{play_label}",
                onclick: move |evt| on_toggle.call(evt),
                "{play_label}"
            }
            button {
                class: "btn ghost",
                style: "width:48px; height:48px; padding:0; border-radius:999px; font-family:var(--mono); font-size:11px;",
                r#type: "button",
                "data-testid": "listen-skip-forward",
                "aria-label": "Forward 30 seconds",
                onclick: move |evt| on_skip_forward.call(evt),
                "+30"
            }
            button {
                class: "btn ghost",
                style: "min-width:48px; height:40px; padding:0 12px; border-radius:999px; font-family:var(--mono); font-size:12px;",
                r#type: "button",
                "data-testid": "listen-rate",
                "aria-label": "Playback speed",
                onclick: move |evt| on_rate.call(evt),
                "{rate_label}"
            }
        }
    }
}
