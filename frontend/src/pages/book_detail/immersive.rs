//! The Immersive Read CTA — the dual-format-only action that opens the EPUB
//! reader with the audiobook docked. Compiled on every target (unlike the
//! web-only [`super::hero`]): the web hero row and the mobile CTA column both
//! render it, with a per-target click handler retargeting that platform's
//! playback context before navigating to `/read`.

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

/// Immersive Read CTA (web): opens the reader and docks the audiobook player
/// together. Clicking retargets the app-wide [`crate::PlaybackState`] at this
/// book — the App-level audio bootstrap then loads its manifest and the `/read`
/// route's [`crate::pages::MiniDock`] surfaces (paused, at the resume position)
/// once book + uuid resolve — then navigates to the reader. Playback itself
/// starts on the first transport action, not on load. The playback context and
/// navigator are read inside the handler (not as render-time hooks) so the
/// button renders under SSR and in unit tests without a provider, keeping
/// hydration parity (rule 07).
#[cfg(not(feature = "mobile"))]
#[component]
pub(super) fn BdImmersiveButton(uuid: String) -> Element {
    let on_click = move |_: MouseEvent| {
        let playback = consume_context::<crate::PlaybackState>();
        // Retarget only when the book differs, mirroring the listen page: clear
        // the previous book's metadata/error and flag loading first so the dock
        // can't flash the old book under the new reader before the driver reloads.
        let mut uuid_sig = playback.uuid;
        if uuid_sig.peek().as_deref() != Some(uuid.as_str()) {
            let mut book_sig = playback.book;
            let mut error_sig = playback.error;
            let mut loading_sig = playback.loading;
            book_sig.set(None);
            error_sig.set(None);
            loading_sig.set(true);
            uuid_sig.set(Some(uuid.clone()));
        }
        dioxus_router::navigator().push(Route::BookRead { uuid: uuid.clone() });
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

/// Immersive Read CTA (mobile): opens the reader with the audiobook docked.
/// Clicking retargets the app-wide [`crate::pages::MobilePlayback`] at this
/// book — the app-root `MobileAudioHost` then loads its manifest and installs
/// the audio surface, and the `/read` route's docked
/// [`crate::pages::MobileMiniPlayer`] surfaces once the view resolves — then
/// navigates to the reader. Same handler-time context reads as the web
/// variant so the button renders in unit tests without a provider.
#[cfg(feature = "mobile")]
#[component]
pub(super) fn BdImmersiveButton(uuid: String) -> Element {
    let on_click = move |_: MouseEvent| {
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
        dioxus_router::navigator().push(Route::BookRead { uuid: uuid.clone() });
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
