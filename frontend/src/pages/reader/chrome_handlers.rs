//! Top-bar / page-turn / keydown event handlers for `BookReadPage`.
//! Extracted so the parent component reads as setup → render rather than
//! a 60-line block of cfg-gated closures. Each handler bridges signal
//! state with the (web-only) JS calls and (non-mobile) router navigation.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;

use omnibus_shared::Highlight;

use super::selection::SelectionData;

/// Overlay-dismissal signals threaded into the Escape handler so a single
/// Escape press peels back the topmost open overlay before navigating away.
#[derive(Copy, Clone)]
pub(super) struct OverlaySignals {
    pub show_aa: Signal<bool>,
    pub show_toc: Signal<bool>,
    pub show_highlights: Signal<bool>,
    pub note_target: Signal<Option<Highlight>>,
}

/// Build the `(on_back, on_prev, on_next, on_keydown)` handlers consumed
/// by `ReaderLayout`. Each handler closes over the passed signals and
/// (where applicable) the router navigator.
pub(super) fn install_chrome_handlers(
    selection: Signal<Option<SelectionData>>,
    overlays: OverlaySignals,
) -> (
    EventHandler<MouseEvent>,
    EventHandler<MouseEvent>,
    EventHandler<MouseEvent>,
    EventHandler<KeyboardEvent>,
) {
    #[cfg(not(feature = "mobile"))]
    let nav = use_navigator();
    let on_back = EventHandler::new(move |_: MouseEvent| {
        #[cfg(not(feature = "mobile"))]
        nav.go_back();
    });
    let on_prev = EventHandler::new(move |_: MouseEvent| advance_page(selection, Direction::Prev));
    let on_next = EventHandler::new(move |_: MouseEvent| advance_page(selection, Direction::Next));
    let on_keydown = EventHandler::new(move |evt: KeyboardEvent| {
        handle_keydown(
            evt,
            selection,
            overlays,
            #[cfg(not(feature = "mobile"))]
            nav,
        );
    });
    (on_back, on_prev, on_next, on_keydown)
}

/// Whether a page-turn was triggered backwards or forwards.
#[derive(Clone, Copy)]
enum Direction {
    Prev,
    Next,
}

/// Clear any live selection and ask the epub.js glue to page in `dir`.
#[cfg(feature = "web")]
fn advance_page(mut selection: Signal<Option<SelectionData>>, dir: Direction) {
    selection.set(None);
    match dir {
        Direction::Prev => super::reader_call("prev", ""),
        Direction::Next => super::reader_call("next", ""),
    }
}

/// Non-web stub: the JS glue only exists in the browser, so paging is a no-op.
#[cfg(not(feature = "web"))]
fn advance_page(_: Signal<Option<SelectionData>>, _: Direction) {}

/// Reader keyboard map: arrows page the book; Escape peels back the topmost
/// overlay (selection → note composer → contents → highlights → AA panel)
/// before navigating back.
fn handle_keydown(
    evt: KeyboardEvent,
    selection: Signal<Option<SelectionData>>,
    overlays: OverlaySignals,
    #[cfg(not(feature = "mobile"))] nav: dioxus_router::Navigator,
) {
    match evt.key() {
        Key::ArrowLeft => {
            evt.prevent_default();
            #[cfg(feature = "web")]
            super::reader_call("prev", "");
        }
        Key::ArrowRight => {
            evt.prevent_default();
            #[cfg(feature = "web")]
            super::reader_call("next", "");
        }
        Key::Escape => {
            evt.prevent_default();
            let mut selection = selection;
            let mut show_aa = overlays.show_aa;
            let mut show_toc = overlays.show_toc;
            let mut show_highlights = overlays.show_highlights;
            let mut note_target = overlays.note_target;
            if selection.read().is_some() {
                selection.set(None);
            } else if note_target.read().is_some() {
                note_target.set(None);
            } else if *show_toc.read() {
                show_toc.set(false);
            } else if *show_highlights.read() {
                show_highlights.set(false);
            } else if *show_aa.read() {
                show_aa.set(false);
            } else {
                #[cfg(not(feature = "mobile"))]
                nav.go_back();
            }
        }
        _ => {}
    }
}
