//! The Immersive Read CTA — the dual-format-only action that opens the EPUB
//! reader with the audiobook docked. Compiled on every target (unlike the
//! web-only [`super::hero`]): the web hero row, the mobile CTA column, and
//! the landing Continue hero's linked-card pill all trigger it, through one
//! shared retarget-and-navigate handler with a per-target body.

use dioxus::prelude::*;

use crate::Route;

/// The book+soundwave glyph on the Immersive Read CTA. Factored out so the
/// per-target buttons share identical markup.
fn bd_immersive_mark() -> Element {
    rsx! {
        span { class: "bd-immersive-mark", aria_hidden: "true",
            svg {
                width: "17",
                height: "17",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                "stroke-width": "1.7",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                path { d: "M4 5.2A2 2 0 0 1 6 4h4.2a1.8 1.8 0 0 1 1.8 1.8V18a1.6 1.6 0 0 0-1.6-1.6H6A2 2 0 0 1 4 14.4V5.2z" }
                path { d: "M15.5 8v8M18.5 6v12M21 9.5v5" }
            }
        }
    }
}

/// Retarget the app-wide playback context at `uuid` and open the reader —
/// the Immersive Read action (web). The App-level audio bootstrap loads the
/// book's manifest and the `/read` route's [`crate::pages::MiniDock`]
/// surfaces (paused, at the resume position) once book + uuid resolve.
/// Playback itself starts on the first transport action, not on load. The
/// playback context and navigator are read here at click time (not as
/// render-time hooks) so callers render under SSR and in unit tests without
/// a provider, keeping hydration parity (rule 07).
#[cfg(not(feature = "mobile"))]
pub(crate) fn retarget_and_open_immersive(uuid: String) {
    let playback = consume_context::<crate::PlaybackState>();
    // Retarget only when the book differs, mirroring the listen page: clear
    // the previous book's metadata/error and flag loading first so the dock
    // can't flash the old book under the new reader before the driver reloads.
    let mut uuid_sig = playback.uuid;
    let mut file_sig = playback.file_id;
    // Default file selection, published before the uuid so the driver
    // reads it when the load kicks off (mirrors the mobile variant and
    // `use_retarget_playback`). Without this a stale picker selection
    // from a previously-played book would override the bootstrap's
    // progress-row file resolution — or 404 the new book's manifest.
    file_sig.set(None);
    // Re-resolve unless the SAME book is actively playing: the old
    // same-uuid short-circuit kept whatever in-memory state the dock
    // held, which on an idle target can be stale against the stored
    // rows (issue #1954). A live playback is the one state that must
    // not be interrupted — everything else re-runs the boot's resume
    // + follow resolution.
    let same_book = uuid_sig.peek().as_deref() == Some(uuid.as_str());
    let live = same_book && *playback.playing.peek();
    if !live {
        let mut book_sig = playback.book;
        let mut error_sig = playback.error;
        let mut loading_sig = playback.loading;
        book_sig.set(None);
        error_sig.set(None);
        loading_sig.set(true);
        if same_book {
            // Same uuid: the driver keys on the value CHANGING, and a
            // None→Some pair inside one handler collapses to
            // no-change — which left the dock cleared but never
            // rebooted (the "Immersive Read doesn't open the player"
            // regression). Commit the None now; the Some lands from a
            // spawned task after this render.
            uuid_sig.set(None);
            let uuid_next = uuid.clone();
            let mut uuid_sig_next = uuid_sig;
            spawn(async move {
                uuid_sig_next.set(Some(uuid_next));
            });
        } else {
            uuid_sig.set(Some(uuid.clone()));
        }
    }
    dioxus_router::navigator().push(Route::BookRead { uuid });
}

/// Retarget the app-wide playback context at `uuid` and open the reader —
/// the Immersive Read action (mobile). Retargets
/// [`crate::pages::MobilePlayback`]: the app-root `MobileAudioHost` loads
/// the book's manifest and installs the audio surface, and the `/read`
/// route's docked [`crate::pages::MobileMiniPlayer`] surfaces once the view
/// resolves. Same click-time context reads as the web variant.
#[cfg(feature = "mobile")]
pub(crate) fn retarget_and_open_immersive(uuid: String) {
    let ctx = consume_context::<crate::pages::MobilePlayback>();
    let mut file_sig = ctx.file_id;
    let mut uuid_sig = ctx.uuid;
    // Default file selection (the host picks the first audio file), and
    // publish it before the uuid so the load reads the right file —
    // mirroring `MobilePlayer`'s retarget effect. Retarget only when the
    // book differs so re-entering the playing book is seamless.
    file_sig.set(None);
    if uuid_sig.peek().as_deref() != Some(uuid.as_str()) {
        uuid_sig.set(Some(uuid.clone()));
    }
    dioxus_router::navigator().push(Route::BookRead { uuid });
}

/// Immersive Read CTA: opens the reader and docks the audiobook player
/// together, via [`retarget_and_open_immersive`]. Renders identically on
/// every target; only the handler internals differ per platform.
#[component]
pub(super) fn BdImmersiveButton(uuid: String) -> Element {
    let on_click = {
        let uuid = uuid.clone();
        move |_: MouseEvent| retarget_and_open_immersive(uuid.clone())
    };
    rsx! {
        button {
            class: "btn lg bd-immersive-cta",
            r#type: "button",
            "data-testid": "immersive-read",
            title: "Open the ereader and audiobook together, kept in sync",
            onclick: on_click,
            {bd_immersive_mark()}
            "Immersive Read"
        }
    }
}
