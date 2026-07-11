//! Top-bar / page-turn / keydown event handlers for `BookReadPage`.
//! Extracted so the parent component reads as setup → render rather than
//! a 60-line block of cfg-gated closures. Each handler bridges signal
//! state with the (web/mobile) JS glue calls and router navigation.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::Highlight;

use super::selection::SelectionData;

/// Overlay-dismissal signals threaded into the Escape handler so a single
/// Escape press peels back the topmost open overlay before navigating away.
#[derive(Copy, Clone)]
pub(super) struct OverlaySignals {
    pub show_aa: Signal<bool>,
    pub show_toc: Signal<bool>,
    pub show_search: Signal<bool>,
    pub show_highlights: Signal<bool>,
    pub show_bookmarks: Signal<bool>,
    pub show_annotations: Signal<bool>,
    pub note_target: Signal<Option<Highlight>>,
    pub quote_target: Signal<Option<Highlight>>,
}

/// Build the `(on_back, on_prev, on_next, on_keydown)` handlers consumed
/// by `ReaderLayout`. Each handler closes over the passed signals and
/// (where applicable) the router navigator.
pub(super) fn install_chrome_handlers(
    uuid: String,
    selection: Signal<Option<SelectionData>>,
    overlays: OverlaySignals,
) -> (
    EventHandler<MouseEvent>,
    EventHandler<MouseEvent>,
    EventHandler<MouseEvent>,
    EventHandler<KeyboardEvent>,
) {
    let nav = use_navigator();
    let back_uuid = uuid.clone();
    let on_back = EventHandler::new(move |_: MouseEvent| back_to_book(nav, &back_uuid));
    let on_prev = EventHandler::new(move |_: MouseEvent| advance_page(selection, Direction::Prev));
    let on_next = EventHandler::new(move |_: MouseEvent| advance_page(selection, Direction::Next));
    let on_keydown = EventHandler::new(move |evt: KeyboardEvent| {
        handle_keydown(evt, selection, overlays, nav, &uuid);
    });
    (on_back, on_prev, on_next, on_keydown)
}

/// Leave the reader: back through history when there is any, else route to
/// the book's detail page. The native shell has no browser history to lean
/// on when it cold-starts into `/read/:uuid`, and its `go_back` was a no-op
/// for years — the explicit detail-page fallback matches the audiobook
/// player's back affordance.
fn back_to_book(nav: dioxus_router::Navigator, uuid: &str) {
    if nav.can_go_back() {
        nav.go_back();
    } else {
        let _ = nav.push(crate::Route::BookDetail {
            uuid: uuid.to_string(),
        });
    }
}

/// Whether a page-turn was triggered backwards or forwards.
#[derive(Clone, Copy)]
enum Direction {
    Prev,
    Next,
}

/// Clear any live selection and ask the epub.js glue to page in `dir`.
#[cfg(any(feature = "web", feature = "mobile"))]
fn advance_page(mut selection: Signal<Option<SelectionData>>, dir: Direction) {
    selection.set(None);
    match dir {
        Direction::Prev => super::reader_call("prev", ""),
        Direction::Next => super::reader_call("next", ""),
    }
}

/// SSR stub: the JS glue only exists in the WebView, so paging is a no-op.
#[cfg(not(any(feature = "web", feature = "mobile")))]
fn advance_page(_: Signal<Option<SelectionData>>, _: Direction) {}

/// Reader keyboard map: arrows page the book; Escape peels back the topmost
/// overlay (selection → note composer → contents → highlights → AA panel)
/// before navigating back.
fn handle_keydown(
    evt: KeyboardEvent,
    selection: Signal<Option<SelectionData>>,
    overlays: OverlaySignals,
    nav: dioxus_router::Navigator,
    uuid: &str,
) {
    match evt.key() {
        Key::ArrowLeft => {
            evt.prevent_default();
            #[cfg(any(feature = "web", feature = "mobile"))]
            super::reader_call("prev", "");
        }
        Key::ArrowRight => {
            evt.prevent_default();
            #[cfg(any(feature = "web", feature = "mobile"))]
            super::reader_call("next", "");
        }
        Key::Escape => {
            evt.prevent_default();
            let mut selection = selection;
            let mut show_aa = overlays.show_aa;
            let mut show_toc = overlays.show_toc;
            let mut show_search = overlays.show_search;
            let mut show_highlights = overlays.show_highlights;
            let mut show_bookmarks = overlays.show_bookmarks;
            let mut show_annotations = overlays.show_annotations;
            let mut note_target = overlays.note_target;
            let mut quote_target = overlays.quote_target;
            if selection.read().is_some() {
                selection.set(None);
            } else if note_target.read().is_some() {
                note_target.set(None);
            } else if quote_target.read().is_some() {
                quote_target.set(None);
            } else if *show_search.read() {
                show_search.set(false);
            } else if *show_toc.read() {
                show_toc.set(false);
            } else if *show_highlights.read() {
                show_highlights.set(false);
            } else if *show_bookmarks.read() {
                show_bookmarks.set(false);
            } else if *show_annotations.read() {
                show_annotations.set(false);
            } else if *show_aa.read() {
                show_aa.set(false);
            } else {
                back_to_book(nav, uuid);
            }
        }
        _ => {}
    }
}
