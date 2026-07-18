//! Persistent mobile mini-player.
//!
//! Docked above the tab bar via `ScreenLayout`; reads [`super::state::MobilePlayback`]
//! to render transport controls while an audiobook is loaded.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::components::atrium::Cover;
use crate::contexts::use_server_url;
use crate::Route;

use super::state::use_mobile_playback;
use super::view::{chapter_index_for_elapsed, format_hms, remaining_at_rate};
use super::{cover_src, interop};

/// Renders the docked mini-player, or nothing when no audiobook is loaded.
#[component]
pub fn MobileMiniPlayer() -> Element {
    let ctx = use_mobile_playback();
    let server_url = use_server_url();

    let Some(view) = (ctx.view)() else {
        return rsx! {};
    };
    // HLS books render an unsupported message in the full player and never
    // start playback — nothing for a mini transport to control.
    if (ctx.unsupported)() {
        return rsx! {};
    }
    let uuid = (ctx.uuid)().unwrap_or_default();
    let elapsed = (ctx.elapsed)();
    let playing = (ctx.playing)();
    let duration = if (ctx.duration)() > 0.0 {
        (ctx.duration)()
    } else {
        view.total_duration
    };
    let pct = if duration > 0.0 {
        (elapsed / duration).clamp(0.0, 1.0) * 100.0
    } else {
        0.0
    };
    let remaining = remaining_at_rate((duration - elapsed).max(0.0), (ctx.rate)());
    let subtitle = if view.chapters.is_empty() {
        format!("{} left", format_hms(remaining))
    } else {
        let ch_no = chapter_index_for_elapsed(&view.chapters, elapsed) + 1;
        format!("Ch. {ch_no} \u{00b7} {} left", format_hms(remaining))
    };
    let accent_style = view
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();

    let mut playing_sig = ctx.playing;
    let on_toggle = move |_| {
        interop::toggle();
        let now = *playing_sig.peek();
        playing_sig.set(!now);
    };

    rsx! {
        div { class: "m-mini", style: "{accent_style}", "data-testid": "mobile-miniplayer",
            div { class: "m-mini-progress", div { style: "width:{pct}%" } }
            div { class: "m-mini-row",
                Link {
                    to: Route::BookListen { uuid: uuid.clone() },
                    class: "m-mini-main",
                    "aria-label": "Open player for {view.title}",
                    span { class: "m-mini-cover",
                        Cover {
                            book: view.book.clone(),
                            src_override: cover_src(&view.book, &uuid, &server_url),
                            sizes: Some("40px".to_string()),
                        }
                    }
                    span { class: "m-mini-meta",
                        span { class: "m-mini-title", "{view.title}" }
                        span { class: "m-mini-sub", "{subtitle}" }
                    }
                }
                button {
                    r#type: "button", class: "m-mini-skip mono",
                    "data-testid": "mini-skip-back", "aria-label": "Back 30 seconds",
                    onclick: move |_| interop::skip(-30.0),
                    "-30"
                }
                button {
                    r#type: "button", class: "m-mini-play",
                    "data-testid": "mini-toggle",
                    "aria-label": if playing { "Pause" } else { "Play" },
                    onclick: on_toggle,
                    if playing {
                        span { class: "m-mini-pause", span {} span {} }
                    } else {
                        span { class: "m-mini-tri" }
                    }
                }
                button {
                    r#type: "button", class: "m-mini-skip mono",
                    "data-testid": "mini-skip-forward", "aria-label": "Forward 30 seconds",
                    onclick: move |_| interop::skip(30.0),
                    "+30"
                }
                Link {
                    to: Route::BookListen { uuid: uuid.clone() },
                    class: "m-mini-expand",
                    "data-testid": "mini-expand",
                    "aria-label": "Expand to full player",
                    svg {
                        width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
                        stroke: "currentColor", stroke_width: "1.9",
                        stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M6 15l6-6 6 6" }
                    }
                }
            }
        }
    }
}
