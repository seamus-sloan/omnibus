//! Immersive audiobook player with direct-play and HLS fallback.
//!
//! Web renders a full-screen two-column "Now playing" surface — cover, title,
//! and author on the left; scrub bar and transport on the right. Mobile renders
//! the single-column [`mobile::MobilePlayer`] adaptation. See [`BookListenPage`]
//! for the startup sequence and position-sync details.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::Link;

#[cfg(not(feature = "mobile"))]
use crate::{use_playback, Route};

#[cfg(not(feature = "mobile"))]
mod bookmarks;
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
mod mini_dock;
#[cfg(feature = "mobile")]
mod mobile;
#[cfg(not(feature = "mobile"))]
mod overlays;
#[cfg(not(feature = "mobile"))]
mod ready_player;
#[cfg(not(feature = "mobile"))]
mod sleep;
#[cfg(not(feature = "mobile"))]
mod sleep_panel;
#[cfg(not(feature = "mobile"))]
mod speed_panel;
#[cfg(not(feature = "mobile"))]
mod stage;

#[cfg(not(feature = "mobile"))]
use ready_player::{PlaybackSignals, ReadyPlayer};

// App-root re-exports: the audio element + dock are mounted by `App` /
// `ScreenLayout`, and the playback driver is called once from `App`.
#[cfg(feature = "web")]
pub(crate) use bootstrap::install_audio_bootstrap;
#[cfg(not(feature = "mobile"))]
pub(crate) use controls::AudioElement;
#[cfg(not(feature = "mobile"))]
pub(crate) use mini_dock::{dock_is_active, MiniDock};
// Mobile app-root pieces: the playback context (provided by `App`), the
// render-less audio host, and the persistent mini-player (`ScreenLayout`).
#[cfg(feature = "mobile")]
pub(crate) use mobile::{mobile_dock_is_active, MobileAudioHost, MobileMiniPlayer, MobilePlayback};

/// Renders the audiobook listen page for the book with the given uuid.
///
/// Startup sequence: `GET /api/audiobooks/{uuid}/manifest` returns either
/// `{mode: "direct", parts}` (m4b/m4a/mp3/aac — instant) or `{mode: "hls",
/// playlist_url}` (everything else). Direct mode chains per-part
/// `<audio src>` URLs with auto-advance on `ended` via the JS shim, using a
/// cumulative sum of per-part durations as the timeline; HLS mode falls back
/// to the legacy `/status` poll + hls.js attach, rendering an error overlay
/// if `/status` reports `state: "failed"` instead of polling forever.
///
/// Position lives in [`crate::audiobook_progress`] — localStorage on web,
/// in-memory on mobile, no-op under SSR. Writes both there AND
/// fire-and-forget POST `/api/rpc/progress` with `format: "audio"` +
/// `audio_position_seconds`, so a position written on one device syncs
/// forward on the next open. Across direct-mode parts the JS shim reports
/// absolute (cross-part) seconds so the same shape works for both modes.
#[component]
pub fn BookListenPage(uuid: String, file_id: Option<i64>) -> Element {
    #[cfg(feature = "mobile")]
    {
        return rsx! {
            mobile::MobilePlayer { uuid, file_id }
        };
    }

    #[cfg(not(feature = "mobile"))]
    {
        // Web resolves the picker's `?file_id=` from `window.location` in the
        // app-root playback bootstrap, so the routed prop is unused here.
        let _ = file_id;
        let playback = use_playback();

        // Point the app-wide player at this route's book. The App-level
        // driver (install_audio_bootstrap) reacts to `uuid` and does the
        // fetch + manifest init; we only retarget it, and only when it
        // differs so re-entering an already-playing book is seamless.
        let route_uuid = uuid.clone();
        use_effect(use_reactive!(|route_uuid| {
            let mut uuid_sig = playback.uuid;
            let mut loading_sig = playback.loading;
            let mut book_sig = playback.book;
            let mut error_sig = playback.error;
            if uuid_sig.peek().as_deref() != Some(route_uuid.as_str()) {
                // Clear the previous book's app-global metadata before
                // retargeting so a re-render can't show the old book (or its
                // error) under the new URL until the App-level driver reloads.
                // The driver also clears these defensively on the uuid swap.
                book_sig.set(None);
                error_sig.set(None);
                loading_sig.set(true);
                uuid_sig.set(Some(route_uuid.clone()));
            }
        }));

        let active = playback.uuid.read().as_deref() == Some(uuid.as_str());

        if active {
            if let Some(msg) = playback.error.read().clone() {
                return rsx! {
                    p { role: "alert", class: "subtitle", "{msg}" }
                    Link { to: Route::Landing {}, class: "btn", "Back to library" }
                };
            }
            if let Some(b) = playback.book.read().clone() {
                return rsx! {
                    ReadyPlayer {
                        book: b,
                        uuid: uuid.clone(),
                        signals: PlaybackSignals {
                            duration: playback.duration,
                            elapsed: playback.elapsed,
                            playing: playback.playing,
                            rate: playback.rate,
                            rate_error: playback.rate_error,
                            volume: playback.volume,
                            hls_ready: playback.hls_ready,
                        },
                        playback_failed: playback.playback_failed,
                        chapters: playback.chapters,
                    }
                };
            }
            if !*playback.loading.read() {
                return rsx! {
                    p { class: "subtitle", "Audiobook not found." }
                    Link { to: Route::Landing {}, class: "btn", "Back to library" }
                };
            }
        }

        rsx! { p { class: "subtitle", "Loading\u{2026}" } }
    }
}
