//! Persistent mini-dock audiobook bar, rendered by the web
//! [`crate::ScreenLayout`] on every main page and by the immersive
//! `/read` route (which has no transport of its own); stays absent
//! from `/listen`, whose full player already owns one. Reads the
//! app-wide [`crate::PlaybackState`] and drives transport through the
//! shared `helpers::audio_call` seam.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;

use super::controls::VolumeControl;
use super::helpers::format_hms;
use super::ready_player::chapter_index_for_elapsed;
use super::stage::chapter_sub_text;
use crate::components::atrium::Cover;
use crate::{use_playback, Route};

/// Progress fill percent (0–100) for the dock's mini progress bar. Returns 0
/// when the duration is unknown or `elapsed` is non-finite so the bar never
/// renders a NaN width.
pub(super) fn progress_pct(elapsed: f64, duration: f64) -> f64 {
    if duration <= 0.0 || !elapsed.is_finite() {
        return 0.0;
    }
    ((elapsed / duration) * 100.0).clamp(0.0, 100.0)
}

/// Whether the mini-dock has both a loaded book and a resolved uuid — the
/// two pieces of playback state its active transport bar needs. `None`
/// means it should render the empty host instead: `book` alone isn't
/// enough because the Expand link would point at an empty `/listen/` route
/// (e.g. mid-dismiss, when `book` is set but `uuid` has momentarily
/// cleared).
pub(super) fn dock_active_state(
    book: Option<omnibus_shared::EbookMetadata>,
    uuid: Option<String>,
) -> Option<(omnibus_shared::EbookMetadata, String)> {
    match (book, uuid) {
        (Some(book), Some(uuid)) => Some((book, uuid)),
        _ => None,
    }
}

/// Persistent bottom dock. Renders only its empty host wrapper until an
/// audiobook is loaded, keeping SSR and first-WASM-paint markup identical
/// (`book` is `None` on both).
#[component]
pub fn MiniDock() -> Element {
    let playback = use_playback();

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
    let chapters = (playback.chapters)();

    let idx = chapter_index_for_elapsed(&chapters, elapsed);
    let chapter_sub = chapter_sub_text(&chapters, idx);
    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());

    rsx! {
        div { class: "mini-dock-host",
            div {
                class: "mini-dock",
                "data-testid": "mini-dock",
                role: "region",
                "aria-label": "Now playing",

                Link {
                    to: Route::BookListen { uuid: uuid.clone() },
                    class: "mini-dock-now",
                    "data-testid": "mini-dock-expand",
                    "aria-label": "Expand player",
                    div { class: "mini-dock-cover", Cover { book: book.clone() } }
                    div { class: "mini-dock-meta",
                        div { class: "mini-dock-title", "data-testid": "mini-dock-title", "{title}" }
                        if let Some(sub) = chapter_sub {
                            div { class: "mini-dock-sub", "{sub}" }
                        }
                    }
                }

                MiniDockProgress { elapsed: elapsed, duration: duration }
                MiniDockControls { playing: playing, rate: rate }
                MiniDockVolume {}
                MiniDockActions { uuid: uuid.clone() }
            }
        }
    }
}

/// Compact volume slider, rendered as a sibling of the dock's action
/// buttons so it doesn't crowd the transport cluster in the narrow bar.
/// Reads/writes the same [`crate::PlaybackState::volume`] signal as the
/// full player's slider via `helpers::apply_volume`.
#[component]
fn MiniDockVolume() -> Element {
    let playback = use_playback();
    let volume = (playback.volume)();
    let on_volume = move |v: f64| {
        let mut vol_sig = playback.volume;
        super::helpers::apply_volume(&mut vol_sig, v);
    };
    rsx! {
        VolumeControl { volume, on_volume, compact: true }
    }
}

/// Elapsed/total labels and progress bar for the dock.
#[component]
fn MiniDockProgress(elapsed: f64, duration: f64) -> Element {
    let fill = format!("width: {:.1}%", progress_pct(elapsed, duration));
    let elapsed_label = format_hms(elapsed);
    let total_label = format_hms(duration);
    rsx! {
        div { class: "mini-dock-track",
            div { class: "mini-dock-times",
                span { "{elapsed_label}" }
                span { "{total_label}" }
            }
            div { class: "pbar",
                i { style: "{fill}" }
            }
        }
    }
}

/// Transport cluster: skip-back, play/pause, skip-forward, rate badge.
#[component]
fn MiniDockControls(playing: bool, rate: f64) -> Element {
    let play_label = if playing { "Pause" } else { "Play" }.to_string();
    let rate_label = format!("{rate:.2}\u{00d7}");
    let on_toggle = super::helpers::on_toggle_playback();
    let on_skip_back = super::helpers::on_skip_back_30();
    let on_skip_forward = super::helpers::on_skip_forward_30();
    rsx! {
        div { class: "mini-dock-controls",
            button {
                class: "mini-dock-btn mini-dock-skip",
                r#type: "button",
                "data-testid": "mini-dock-skip-back",
                "aria-label": "Back 30 seconds",
                onclick: on_skip_back,
                "-30"
            }
            button {
                class: "mini-dock-play",
                r#type: "button",
                "data-testid": "mini-dock-toggle",
                "aria-label": "{play_label}",
                onclick: on_toggle,
                if playing {
                    div { class: "mini-dock-ico-pause",
                        div { class: "mini-dock-ico-pause-bar" }
                        div { class: "mini-dock-ico-pause-bar" }
                    }
                } else {
                    div { class: "mini-dock-ico-play" }
                }
            }
            button {
                class: "mini-dock-btn mini-dock-skip",
                r#type: "button",
                "data-testid": "mini-dock-skip-forward",
                "aria-label": "Forward 30 seconds",
                onclick: on_skip_forward,
                "+30"
            }
            span { class: "mini-dock-rate", "data-testid": "mini-dock-rate", "{rate_label}" }
        }
    }
}

/// Expand link + dismiss button; dismiss clears [`crate::PlaybackState`] via `use_playback`.
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
                to: Route::BookListen { uuid: uuid.clone() },
                class: "mini-dock-btn mini-dock-expand-btn",
                "data-testid": "mini-dock-expand-btn",
                "\u{2191} Expand"
            }
            button {
                class: "mini-dock-btn mini-dock-dismiss",
                r#type: "button",
                "data-testid": "mini-dock-dismiss",
                "aria-label": "Stop and close player",
                onclick: on_dismiss,
                "\u{00d7}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::EbookMetadata;

    use super::{dock_active_state, progress_pct};

    #[test]
    fn progress_pct_is_zero_when_duration_unknown() {
        assert_eq!(progress_pct(10.0, 0.0), 0.0);
        assert_eq!(progress_pct(10.0, -5.0), 0.0);
    }

    #[test]
    fn progress_pct_computes_midpoint() {
        assert!((progress_pct(50.0, 200.0) - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_pct_clamps_above_full() {
        assert_eq!(progress_pct(300.0, 200.0), 100.0);
    }

    #[test]
    fn progress_pct_treats_non_finite_elapsed_as_zero() {
        assert_eq!(progress_pct(f64::NAN, 100.0), 0.0);
        assert_eq!(progress_pct(f64::INFINITY, 100.0), 0.0);
    }

    // Mirrors MiniDock's "renders empty host vs. active bar" branch without needing a full component render.
    #[test]
    fn dock_active_state_is_none_when_nothing_is_playing() {
        assert_eq!(dock_active_state(None, None), None);
    }

    #[test]
    fn dock_active_state_is_none_when_uuid_has_not_resolved_yet() {
        let book = EbookMetadata {
            title: Some("Test Audiobook".into()),
            ..Default::default()
        };
        assert_eq!(dock_active_state(Some(book), None), None);
    }

    #[test]
    fn dock_active_state_is_none_when_book_has_not_loaded_yet() {
        assert_eq!(dock_active_state(None, Some("uuid-1".to_string())), None);
    }

    #[test]
    fn dock_active_state_returns_both_when_book_and_uuid_are_present() {
        let book = EbookMetadata {
            title: Some("Test Audiobook".into()),
            ..Default::default()
        };
        let (got_book, got_uuid) =
            dock_active_state(Some(book.clone()), Some("uuid-1".to_string())).unwrap();
        assert_eq!(got_book, book);
        assert_eq!(got_uuid, "uuid-1");
    }
}
