//! Top-bar / page-turn / keydown event handlers for `BookReadPage`.
//! Extracted so the parent component reads as setup → render rather than
//! a 60-line block of cfg-gated closures. Each handler bridges signal
//! state with the (web-only) JS calls and (non-mobile) router navigation.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;

use super::selection::SelectionData;

/// Build the `(on_back, on_prev, on_next, on_keydown)` handlers consumed
/// by `ReaderLayout`. Each handler closes over the passed signals and
/// (where applicable) the router navigator.
pub(super) fn install_chrome_handlers(
    selection: Signal<Option<SelectionData>>,
    show_aa: Signal<bool>,
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
            show_aa,
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
/// No-op on non-web targets (the JS glue only exists in the browser).
#[cfg_attr(not(feature = "web"), allow(unused_variables))]
fn advance_page(selection: Signal<Option<SelectionData>>, dir: Direction) {
    #[cfg(feature = "web")]
    {
        let mut selection = selection;
        selection.set(None);
        match dir {
            Direction::Prev => super::reader_call("prev", ""),
            Direction::Next => super::reader_call("next", ""),
        }
    }
}

/// Reader keyboard map: arrows page the book; Escape closes a live
/// selection, then the AA panel, then navigates back.
fn handle_keydown(
    evt: KeyboardEvent,
    selection: Signal<Option<SelectionData>>,
    show_aa: Signal<bool>,
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
            let mut show_aa = show_aa;
            if selection.read().is_some() {
                selection.set(None);
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
