//! Post-load player chrome: top bar, hidden `<audio>`, status overlays,
//! and the two-column [`PlayerStage`]. Owns the per-action handlers
//! (back / toggle / skip / seek / rate). Rendered by the orchestrator
//! after the `loading` / `error` / `book.is_none()` gates.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::EbookMetadata;

use super::controls::{AudioElement, TopBar};
use super::helpers::RATE_STEPS;
use super::overlays::{FailedOverlay, PreparingOverlay};
use super::stage::PlayerStage;

/// Render the ready-state player chrome and bind the transport handlers.
/// The signal props are cheap to clone (Copy) — they're forwarded into
/// each closure so a state change re-renders the appropriate sub-tree.
#[component]
pub(super) fn ReadyPlayer(
    book: EbookMetadata,
    duration: Signal<f64>,
    elapsed: Signal<f64>,
    playing: Signal<bool>,
    rate: Signal<f64>,
    hls_ready: Signal<bool>,
    playback_failed: Signal<bool>,
) -> Element {
    let nav = use_navigator();
    let on_back = move |_| {
        nav.go_back();
    };

    let on_toggle = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("toggle", "");
    };
    let on_skip_back = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "-30");
    };
    let on_skip_forward = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "30");
    };
    let on_seek = move |evt: Event<FormData>| {
        if let Ok(_secs) = evt.value().parse::<f64>() {
            #[cfg(feature = "web")]
            super::helpers::audio_call("seek", &_secs.to_string());
        }
    };
    let on_rate = move |_| {
        let cur = rate();
        let next = RATE_STEPS
            .iter()
            .copied()
            .find(|r| *r > cur + f64::EPSILON)
            .unwrap_or(RATE_STEPS[0]);
        rate.set(next);
        crate::audiobook_progress::save_rate(next);
        #[cfg(feature = "web")]
        super::helpers::audio_call("setRate", &next.to_string());
    };

    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Unknown Author".to_string());
    let dur = duration();
    let elapsed_now = elapsed();
    let remaining = (dur - elapsed_now).max(0.0);
    let scrub_max = if dur > 0.0 { dur } else { 1.0 };
    let rate_label = format!("{:.2}\u{00d7}", rate());
    let play_label = if playing() { "Pause" } else { "Play" }.to_string();
    let ready = hls_ready();
    let failed = playback_failed();

    rsx! {
        div {
            class: "player-root",
            style: "display:flex; flex-direction:column; height:100vh; width:100%; background:var(--bg-0);",

            TopBar { on_back }
            AudioElement {}

            if failed {
                FailedOverlay {}
            } else if !ready {
                PreparingOverlay {}
            }

            PlayerStage {
                book: book.clone(),
                title,
                author,
                elapsed: elapsed_now,
                duration: dur,
                remaining,
                scrub_max,
                play_label,
                rate_label,
                on_seek,
                on_toggle,
                on_skip_back,
                on_skip_forward,
                on_rate,
            }
        }
    }
}
