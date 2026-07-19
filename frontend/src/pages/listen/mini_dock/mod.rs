//! Persistent full-width audiobook dock bar ("AudioDockBar"): left cover +
//! title/meta, centered transport (chapter jumps, ±30, play), right cluster
//! of volume / speed / sleep popovers plus expand & close. Rendered by web
//! [`crate::ScreenLayout`] and `/read`, absent on `/listen`; reads
//! [`crate::PlaybackState`] and drives transport via `helpers::audio_call`.

#![cfg(not(feature = "mobile"))]

mod meta;
mod popovers;
#[cfg(test)]
mod tests;
mod transport;
mod volume;

use dioxus::prelude::*;
use dioxus_router::Link;

use super::chapter_nav::chapter_index_for_elapsed;
use super::stage::chapter_sub_text;
use crate::components::atrium::Cover;
use crate::contexts::use_current_user_summary;
use crate::{use_playback, Route};
pub(crate) use meta::dock_is_active;
use meta::{dock_active_state, dock_sub_text, progress_pct};
use popovers::{DockPanel, DockSleep, DockSpeed};
use transport::DockTransport;
use volume::DockVolume;

/// Persistent bottom dock. Renders only its empty host wrapper until an
/// audiobook is loaded, keeping SSR and first-WASM-paint markup identical
/// (`book` is `None` on both).
#[component]
pub fn MiniDock() -> Element {
    let playback = use_playback();
    // Declared unconditionally (before the early return) so hook order stays
    // stable across the empty-host / active-bar branches. `current_user` only
    // drives the speed panel's per-book save; `open_panel` starts closed on
    // SSR and first WASM paint, so neither can cause a hydration mismatch.
    let current_user = use_current_user_summary();
    let mut open_panel = use_signal(|| None::<DockPanel>);

    // Stable host wrapper so hydration node-counting stays consistent; only
    // the inner bar is conditional on both a loaded book and a resolved uuid.
    let Some((book, uuid)) =
        dock_active_state(playback.book.read().clone(), playback.uuid.read().clone())
    else {
        return rsx! { div { class: "mini-dock-host" } };
    };

    let elapsed = (playback.elapsed)();
    let duration = (playback.duration)();
    let playing = (playback.playing)();
    let rate = (playback.rate)();
    let volume = (playback.volume)();
    let chapters = (playback.chapters)();
    let user_id = current_user().map(|user| user.id);

    let idx = chapter_index_for_elapsed(&chapters, elapsed);
    let chapter_sub = chapter_sub_text(&chapters, idx);
    let sub = dock_sub_text(chapter_sub, elapsed, duration, rate);
    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let fill = format!("width: {:.1}%", progress_pct(elapsed, duration));
    // Book-accent theming, mirroring the full player's `accent_style`.
    let accent_style = book
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();

    rsx! {
        div { class: "mini-dock-host",
            // Transparent outside-click catcher — sits below the bar so its
            // own controls stay clickable while a popover is open. Same idiom
            // as the full player's `.lp-scrim`, no document-level listeners.
            if open_panel().is_some() {
                div {
                    class: "mini-dock-scrim",
                    "data-testid": "mini-dock-scrim",
                    onclick: move |_| open_panel.set(None),
                }
            }
            div {
                class: "mini-dock",
                style: "{accent_style}",
                "data-testid": "mini-dock",
                role: "region",
                "aria-label": "Now playing",

                div { class: "mini-dock-prog",
                    i { style: "{fill}" }
                }

                Link {
                    to: Route::BookListen { uuid: uuid.clone(), file_id: None },
                    class: "mini-dock-now",
                    "data-testid": "mini-dock-expand",
                    "aria-label": "Expand player",
                    div { class: "mini-dock-cover", Cover { book: book.clone() } }
                    div { class: "mini-dock-meta",
                        div { class: "mini-dock-title", "data-testid": "mini-dock-title", "{title}" }
                        div { class: "mini-dock-sub", "{sub}" }
                    }
                }

                DockTransport { playing, has_chapters: !chapters.is_empty() }

                span { class: "mini-dock-spacer" }

                DockVolume { open_panel, volume }
                span { class: "mini-dock-div mini-dock-hide" }
                DockSpeed { open_panel, rate, uuid: uuid.clone(), user_id }
                DockSleep { open_panel }
                span { class: "mini-dock-div" }
                MiniDockActions { uuid: uuid.clone() }
            }
        }
    }
}

/// Inline icon actions on the right of the bar: expand to the full player and
/// dismiss. Dismiss clears [`crate::PlaybackState`] via `use_playback`.
#[component]
fn MiniDockActions(uuid: String) -> Element {
    let playback = use_playback();
    let on_dismiss = {
        let mut uuid_sig = playback.uuid;
        let mut book_sig = playback.book;
        let mut playing_set = playback.playing;
        move |_: MouseEvent| {
            // Fully stop the shared element (pause + clear src + reset mode) so a
            // media-key resume can't restart a dismissed book with no visible
            // control. Clear `book` before `uuid` so the dock hides immediately
            // and PlaybackState stays coherent for any other consumer.
            #[cfg(feature = "web")]
            super::helpers::audio_call("stop", "");
            book_sig.set(None);
            playing_set.set(false);
            uuid_sig.set(None);
        }
    };
    rsx! {
        div { class: "mini-dock-actions",
            Link {
                to: Route::BookListen { uuid: uuid.clone(), file_id: None },
                class: "mini-dock-ico",
                "data-testid": "mini-dock-expand-btn",
                "aria-label": "Expand to full player",
                title: "Expand to full player",
                svg {
                    width: "17",
                    height: "17",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.8",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M8 14l-4 4M4 14v4h4M16 10l4-4M20 10V6h-4" }
                }
            }
            button {
                class: "mini-dock-ico",
                r#type: "button",
                "data-testid": "mini-dock-dismiss",
                "aria-label": "Stop and close player",
                title: "Close audio \u{2014} keep reading",
                onclick: on_dismiss,
                svg {
                    width: "17",
                    height: "17",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.8",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M6 6l12 12M18 6L6 18" }
                }
            }
        }
    }
}
