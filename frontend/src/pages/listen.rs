//! F2.3 immersive audiobook player with direct-play + HLS fallback (#339).
//!
//! Renders a full-screen "Now playing" surface: cover + title + author on the
//! left, scrub bar + transport controls on the right.
//!
//! Startup sequence:
//! 1. `GET /api/audiobooks/{uuid}/manifest` returns either
//!    `{mode: "direct", parts}` (m4b/m4a/mp3/aac — instant) or
//!    `{mode: "hls", playlist_url}` (everything else).
//! 2. For direct mode, the JS shim chains per-part `<audio src>` URLs with
//!    auto-advance on `ended`; the book's timeline is a cumulative sum of
//!    per-part durations.
//! 3. For HLS mode, fall back to the legacy `/status` poll + hls.js attach.
//!    If `/status` reports `state: "failed"`, render an error overlay
//!    instead of polling forever (Bug 4 from #338).
//!
//! Position lives in [`crate::audiobook_progress`] — localStorage on web,
//! in-memory on mobile, no-op under SSR. Writes both there AND fire-and-
//! forget POST `/api/rpc/progress` (F2.1) with `format: "audio"` +
//! `audio_position_seconds`, so a position written on one device syncs
//! forward on the next open. Across direct-mode parts the JS shim reports
//! absolute (cross-part) seconds so the same shape works for both modes.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::Link;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::EbookMetadata;

#[cfg(not(feature = "mobile"))]
use crate::{data, use_server_url, Route};

#[cfg(not(feature = "mobile"))]
mod bookmarks_drawer;
#[cfg(feature = "web")]
mod bootstrap;
#[cfg(not(feature = "mobile"))]
mod chapter_map;
#[cfg(not(feature = "mobile"))]
mod chapters_drawer;
#[cfg(not(feature = "mobile"))]
mod controls;
mod helpers;
#[cfg(not(feature = "mobile"))]
mod overlays;
#[cfg(not(feature = "mobile"))]
mod ready_player;
#[cfg(not(feature = "mobile"))]
mod sleep_panel;
#[cfg(not(feature = "mobile"))]
mod speed_panel;
#[cfg(not(feature = "mobile"))]
mod stage;

#[cfg(not(feature = "mobile"))]
use ready_player::ReadyPlayer;

#[component]
pub fn BookListenPage(uuid: String) -> Element {
    #[cfg(feature = "mobile")]
    {
        let _ = uuid;
        return rsx! {
            div { class: "screen",
                p { class: "subtitle", "Audiobook playback on mobile is coming soon." }
            }
        };
    }

    #[cfg(not(feature = "mobile"))]
    {
        let server_url = use_server_url();
        let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
        let mut loading = use_signal(|| true);
        let mut error: Signal<Option<String>> = use_signal(|| None);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let duration = use_signal(|| 0.0_f64);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let elapsed = use_signal(|| 0.0_f64);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let playing = use_signal(|| false);
        let uuid_for_rate = uuid.clone();
        let rate = use_signal(move || crate::audiobook_progress::load_rate(&uuid_for_rate));
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let hls_ready = use_signal(|| false);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let playback_failed = use_signal(|| false);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let chapters: Signal<Vec<omnibus_shared::ChapterInfo>> = use_signal(Vec::new);

        let url = server_url.clone();
        let uuid_for_fetch = uuid.clone();
        use_effect(use_reactive!(|uuid_for_fetch| {
            let url = url.clone();
            let uuid = uuid_for_fetch.clone();
            spawn(async move {
                loading.set(true);
                match data::get_ebook(&url, &uuid).await {
                    Ok(b) => {
                        book.set(b);
                        error.set(None);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                loading.set(false);
            });
        }));

        #[cfg(feature = "web")]
        bootstrap::install_audio_bootstrap(
            uuid.clone(),
            duration,
            elapsed,
            playing,
            hls_ready,
            playback_failed,
            chapters,
        );

        if loading() {
            return rsx! { p { class: "subtitle", "Loading\u{2026}" } };
        }
        if let Some(msg) = error() {
            return rsx! {
                p { role: "alert", class: "subtitle", "{msg}" }
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            };
        }
        let Some(b) = book() else {
            return rsx! {
                p { class: "subtitle", "Audiobook not found." }
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            };
        };

        rsx! {
            ReadyPlayer {
                book: b,
                uuid: uuid.clone(),
                duration,
                elapsed,
                playing,
                rate,
                hls_ready,
                playback_failed,
                chapters,
            }
        }
    }
}
