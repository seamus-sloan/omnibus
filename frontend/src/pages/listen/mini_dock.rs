//! Persistent mini-dock audiobook bar.
//!
//! Rendered inside the web [`crate::ScreenLayout`] so it appears on every
//! main page while an audiobook is loaded, and is naturally absent on the
//! immersive `/listen` and `/read` surfaces (which render without
//! `ScreenLayout`). Reads the app-wide [`crate::PlaybackState`]; the backing
//! `<audio>` element and signals live at the App root so playback continues
//! across navigation. Transport reuses the same `helpers::audio_call` seam as
//! the full player — there is no second JS poke path.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;

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

/// Persistent bottom dock. Renders only its empty host wrapper until an
/// audiobook is loaded, keeping SSR and first-WASM-paint markup identical
/// (`book` is `None` on both).
#[component]
pub fn MiniDock() -> Element {
    let playback = use_playback();
    let book = playback.book.read().clone();

    // Stable host wrapper so hydration node-counting stays consistent; only
    // the inner bar is conditional on a loaded book.
    let Some(book) = book else {
        return rsx! { div { class: "mini-dock-host" } };
    };

    let uuid = playback.uuid.read().clone().unwrap_or_default();

    let elapsed_sig = playback.elapsed;
    let duration_sig = playback.duration;
    let playing_sig = playback.playing;
    let rate_sig = playback.rate;
    let chapters_sig = playback.chapters;

    let elapsed = elapsed_sig();
    let duration = duration_sig();
    let playing = playing_sig();
    let rate = rate_sig();
    let chapters = chapters_sig();

    let idx = chapter_index_for_elapsed(&chapters, elapsed);
    let chapter_sub = chapter_sub_text(&chapters, idx);
    let fill = format!("width: {:.1}%", progress_pct(elapsed, duration));
    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let play_label = if playing { "Pause" } else { "Play" }.to_string();
    let rate_label = format!("{rate:.2}\u{00d7}");
    let elapsed_label = format_hms(elapsed);
    let total_label = format_hms(duration);

    let on_toggle = move |_: MouseEvent| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("toggle", "");
    };
    let on_skip_back = move |_: MouseEvent| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "-30");
    };
    let on_skip_forward = move |_: MouseEvent| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "30");
    };
    let on_dismiss = {
        let mut uuid_sig = playback.uuid;
        let mut book_sig = playback.book;
        let mut playing_set = playback.playing;
        move |_: MouseEvent| {
            #[cfg(feature = "web")]
            super::helpers::audio_call("pause", "");
            uuid_sig.set(None);
            book_sig.set(None);
            playing_set.set(false);
        }
    };

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

                div { class: "mini-dock-track",
                    div { class: "mini-dock-times",
                        span { "{elapsed_label}" }
                        span { "{total_label}" }
                    }
                    div { class: "pbar",
                        i { style: "{fill}" }
                    }
                }

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
    }
}

#[cfg(test)]
mod tests {
    use super::progress_pct;

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
}
