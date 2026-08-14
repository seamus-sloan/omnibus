//! Top-bar / page-turn / keydown event handlers for `BookReadPage`.
//! Extracted so the parent component reads as setup → render rather than
//! a 60-line block of cfg-gated closures. Each handler bridges signal
//! state with the (web/mobile) JS glue calls and router navigation.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::Highlight;

use super::selection::SelectionData;
use super::ReaderStatus;

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

/// The chrome event handlers [`install_chrome_handlers`] builds, named
/// rather than a tuple — five `EventHandler`s in positional order reads as
/// noise at both the definition and every call site.
pub(super) struct ChromeHandlers {
    pub on_back: EventHandler<MouseEvent>,
    pub on_prev: EventHandler<MouseEvent>,
    pub on_next: EventHandler<MouseEvent>,
    pub on_retry: EventHandler<MouseEvent>,
    pub on_keydown: EventHandler<KeyboardEvent>,
}

/// Build the chrome handlers consumed by `ReaderLayout`. Each handler closes
/// over the passed signals and (where applicable) the router navigator.
pub(super) fn install_chrome_handlers(
    uuid: String,
    selection: Signal<Option<SelectionData>>,
    status: Signal<ReaderStatus>,
    overlays: OverlaySignals,
) -> ChromeHandlers {
    let nav = use_navigator();
    let back_uuid = uuid.clone();
    let on_back = EventHandler::new(move |_: MouseEvent| back_to_book(nav, &back_uuid));
    let on_prev = EventHandler::new(move |_: MouseEvent| advance_page(selection, Direction::Prev));
    let on_next = EventHandler::new(move |_: MouseEvent| advance_page(selection, Direction::Next));
    let on_retry = EventHandler::new(move |_: MouseEvent| retry_reader(status));
    let on_keydown = EventHandler::new(move |evt: KeyboardEvent| {
        handle_keydown(evt, selection, overlays, nav, &uuid);
    });
    ChromeHandlers {
        on_back,
        on_prev,
        on_next,
        on_retry,
        on_keydown,
    }
}

/// Leave the reader: always route to the book's detail page. History-walking
/// (`go_back`) returned to wherever the reader was entered from — the landing
/// grid, search, the Continue hero — but the reader's back affordance means
/// "close this book", and its home is the detail page. `replace` keeps the
/// reader itself out of history so the browser back button doesn't bounce
/// between the reader and the detail page.
fn back_to_book(nav: dioxus_router::Navigator, uuid: &str) {
    let _ = nav.replace(crate::Route::BookDetail {
        uuid: uuid.to_string(),
    });
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

/// Recover from a load failure or a wedged page turn: flip the overlay back
/// to `Loading` immediately (rather than waiting on the glue round trip) and
/// ask it to replay the last `init()` verbatim (issue #1895, AC3) — the
/// error overlay's "Retry" affordance.
#[cfg(any(feature = "web", feature = "mobile"))]
fn retry_reader(mut status: Signal<ReaderStatus>) {
    status.set(ReaderStatus::Loading);
    super::reader_call("retry", "");
}

/// SSR stub: no glue to retry against.
#[cfg(not(any(feature = "web", feature = "mobile")))]
fn retry_reader(_: Signal<ReaderStatus>) {}

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
