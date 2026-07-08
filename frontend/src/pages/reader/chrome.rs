//! Reader top-bar chrome: the overlay-toggling `ReaderTopBar`, its stateless
//! `ReaderTopChrome` button row, the epub.js `ReaderViewerStage` mount target,
//! and the circular `ReaderPageTurnButtons` gutters. Compiles on every target.

use dioxus::prelude::*;

use super::signals::ReaderStatus;
use super::ReaderPanelSignals;

/// Display state for [`ReaderTopChrome`]: the titles plus which tools read as
/// active and the highlight badge count. Grouped to keep the row under the cap.
#[derive(Clone, PartialEq)]
struct ReaderChromeState {
    book_title: String,
    chapter_title: String,
    show_aa: bool,
    toc_active: bool,
    search_active: bool,
    highlights_active: bool,
    bookmarks_active: bool,
    highlight_count: usize,
}

/// Click handlers for [`ReaderTopChrome`]: back plus one toggle per tool.
#[derive(Clone, PartialEq)]
struct ReaderChromeHandlers {
    on_back: EventHandler<MouseEvent>,
    on_toggle_aa: EventHandler<MouseEvent>,
    on_toggle_toc: EventHandler<MouseEvent>,
    on_toggle_search: EventHandler<MouseEvent>,
    on_toggle_highlights: EventHandler<MouseEvent>,
    on_toggle_bookmarks: EventHandler<MouseEvent>,
}

/// Owns the mutually-exclusive overlay toggles and renders [`ReaderTopChrome`].
/// Each tool closes every other surface before opening its own.
#[component]
pub(super) fn ReaderTopBar(
    book_title: String,
    chapter_title: String,
    panels: ReaderPanelSignals,
    on_back: EventHandler<MouseEvent>,
) -> Element {
    let show_aa = panels.show_aa;
    let show_toc = panels.show_toc;
    let show_search = panels.show_search;
    let show_highlights = panels.show_highlights;
    let show_bookmarks = panels.show_bookmarks;
    let note_target = panels.note_target;
    let quote_target = panels.quote_target;
    let highlight_count = panels.highlights.read().len();

    // Close every overlay (panel, drawer, composer) — `Fn` + `Copy` so each
    // toggle handler can call it before opening its own surface, keeping the
    // chrome mutually exclusive.
    let close_overlays = move || {
        let mut a = show_aa;
        a.set(false);
        let mut b = show_toc;
        b.set(false);
        let mut c = show_search;
        c.set(false);
        let mut d = show_highlights;
        d.set(false);
        let mut e = show_bookmarks;
        e.set(false);
        let mut f = note_target;
        f.set(None);
        let mut g = quote_target;
        g.set(None);
    };

    rsx! {
        ReaderTopChrome {
            state: ReaderChromeState {
                book_title,
                chapter_title,
                show_aa: show_aa(),
                toc_active: show_toc(),
                search_active: show_search(),
                highlights_active: show_highlights(),
                bookmarks_active: show_bookmarks(),
                highlight_count,
            },
            handlers: ReaderChromeHandlers {
                on_back,
                on_toggle_aa: EventHandler::new(move |_| {
                    let cur = show_aa();
                    close_overlays();
                    let mut s = show_aa;
                    s.set(!cur);
                }),
                on_toggle_toc: EventHandler::new(move |_| {
                    let cur = show_toc();
                    close_overlays();
                    let mut s = show_toc;
                    s.set(!cur);
                }),
                on_toggle_search: EventHandler::new(move |_| {
                    let cur = show_search();
                    close_overlays();
                    let mut s = show_search;
                    s.set(!cur);
                }),
                on_toggle_highlights: EventHandler::new(move |_| {
                    let cur = show_highlights();
                    close_overlays();
                    let mut s = show_highlights;
                    s.set(!cur);
                }),
                on_toggle_bookmarks: EventHandler::new(move |_| {
                    let cur = show_bookmarks();
                    close_overlays();
                    let mut s = show_bookmarks;
                    s.set(!cur);
                }),
            },
        }
    }
}

/// Top navigation bar: back button, title + chapter display, Aa + bookmark tools.
#[component]
fn ReaderTopChrome(state: ReaderChromeState, handlers: ReaderChromeHandlers) -> Element {
    let ReaderChromeState {
        book_title,
        chapter_title,
        show_aa,
        toc_active,
        search_active,
        highlights_active,
        bookmarks_active,
        highlight_count,
    } = state;
    let ReaderChromeHandlers {
        on_back,
        on_toggle_aa,
        on_toggle_toc,
        on_toggle_search,
        on_toggle_highlights,
        on_toggle_bookmarks,
    } = handlers;
    rsx! {
        div {
            class: "rd-top",
            button {
                class: "rd-tool",
                r#type: "button",
                "data-testid": "reader-back",
                "aria-label": "Back to book",
                onclick: on_back,
                svg {
                    width: "19", height: "19", view_box: "0 0 24 24",
                    fill: "none", stroke: "currentColor",
                    stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "M15 5l-7 7 7 7" }
                }
            }
            div {
                class: "rd-title-center",
                span { class: "rd-title-book", "{book_title}" }
                if !chapter_title.is_empty() {
                    span { class: "rd-title-sep", "\u{b7}" }
                    span { class: "rd-title-ch", "{chapter_title}" }
                }
            }
            div {
                style: "display:flex; align-items:center; gap:2px;",
                button {
                    class: if toc_active { "rd-tool on" } else { "rd-tool" },
                    r#type: "button",
                    "data-testid": "reader-toc",
                    "aria-label": "Contents",
                    onclick: on_toggle_toc,
                    svg {
                        width: "19", height: "19", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M4 6h16M4 12h16M4 18h11" }
                    }
                }
                button {
                    class: if search_active { "rd-tool on" } else { "rd-tool" },
                    r#type: "button",
                    "data-testid": "reader-search",
                    "aria-label": "Search in book",
                    onclick: on_toggle_search,
                    svg {
                        width: "19", height: "19", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        circle { cx: "11", cy: "11", r: "6.2" }
                        path { d: "M20 20l-4.2-4.2" }
                    }
                }
                button {
                    class: if show_aa { "rd-tool rd-aa on" } else { "rd-tool rd-aa" },
                    r#type: "button",
                    "data-testid": "reader-aa",
                    "aria-label": "Display settings",
                    onclick: on_toggle_aa,
                    "Aa"
                }
                button {
                    class: if highlights_active { "rd-tool on" } else { "rd-tool" },
                    r#type: "button",
                    "data-testid": "reader-highlights",
                    "aria-label": "Highlights and notes",
                    onclick: on_toggle_highlights,
                    svg {
                        width: "19", height: "19", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M4 19.5l1.6-4 8.2-8.2 3 3-8.2 8.2-4.6 0z" }
                        path { d: "M13.2 6.1l2.7-2.7a1.3 1.3 0 0 1 1.9 0l1.8 1.8a1.3 1.3 0 0 1 0 1.9l-2.7 2.7" }
                    }
                    if highlight_count > 0 {
                        span { class: "rd-badge", "{highlight_count}" }
                    }
                }
                button {
                    class: if bookmarks_active { "rd-tool on" } else { "rd-tool" },
                    r#type: "button",
                    "data-testid": "reader-bookmark",
                    "aria-label": "Bookmarks",
                    onclick: on_toggle_bookmarks,
                    svg {
                        width: "19", height: "19", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M7 4h10v16l-5-3.6L7 20V4z" }
                    }
                }
            }
        }
    }
}

/// epub.js mount target plus loading/error/ready overlay.
#[component]
pub(super) fn ReaderViewerStage(status: ReaderStatus) -> Element {
    rsx! {
        div {
            class: "rd-stage",
            div { id: "omnibus-viewer", class: "rd-viewer", "data-testid": "reader-viewer" }
            match status {
                ReaderStatus::Loading => rsx! {
                    div { class: "rd-overlay", "data-testid": "reader-loading", "Loading\u{2026}" }
                },
                ReaderStatus::Failed => rsx! {
                    div {
                        class: "rd-overlay",
                        "data-testid": "reader-error",
                        role: "alert",
                        "This book couldn\u{2019}t be loaded."
                    }
                },
                ReaderStatus::Ready => rsx! {},
            }
        }
    }
}

/// Left and right circular page-turn gutter buttons.
#[component]
pub(super) fn ReaderPageTurnButtons(
    on_prev: EventHandler<MouseEvent>,
    on_next: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        button {
            class: "rd-turn rd-turn-l",
            r#type: "button",
            "data-testid": "reader-prev",
            "aria-label": "Previous page",
            onclick: on_prev,
            svg {
                width: "20", height: "20", view_box: "0 0 24 24",
                fill: "none", stroke: "currentColor",
                stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M14.5 5l-7 7 7 7" }
            }
        }
        button {
            class: "rd-turn rd-turn-r",
            r#type: "button",
            "data-testid": "reader-next",
            "aria-label": "Next page",
            onclick: on_next,
            svg {
                width: "20", height: "20", view_box: "0 0 24 24",
                fill: "none", stroke: "currentColor",
                stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M9.5 5l7 7-7 7" }
            }
        }
    }
}
