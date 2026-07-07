//! Immersive full-screen EPUB reader. Loads the vendored epub.js +
//! JSZip glue (`window.OmnibusReader`) via `dioxus::document::eval`, streams
//! bytes from cookie-gated `GET /api/ebooks/:uuid/file`, and persists
//! position via [`crate::reader_progress`]. Chrome compiles on every
//! target; the JS interop that mounts a book is web-only.

mod aa_panel;
mod bootstrap;
mod chrome_handlers;
mod highlights;
mod highlights_drawer;
#[cfg(feature = "web")]
mod interop;
mod note_composer;
mod prefs;
mod quote_panel;
mod reader_bookmarks;
mod search_panel;
mod selection;
mod signals;
mod toc_drawer;
mod typography;

use dioxus::prelude::*;

use crate::components::atrium::Theme;

use omnibus_shared::{Highlight, HighlightColor};

use aa_panel::ReaderAaPanel;
use highlights::{spawn_create_highlight, PostCreate};
use highlights_drawer::HighlightsDrawer;
#[cfg(feature = "web")]
use interop::{install_reader_web_interop, InteropSignals};
use note_composer::NoteComposer;
use prefs::init_reader_prefs;
use quote_panel::QuotePanel;
use reader_bookmarks::ReaderBookmarksDrawer;
use search_panel::{SearchPanel, SearchResult};
use selection::{SelectionData, SelectionPopover};
use signals::{format_progress_labels, use_book_metadata, ReaderStatus, RelocateData};
use toc_drawer::{TocDrawer, TocEntry};

const JSZIP_JS: Asset = asset!("/assets/vendor/jszip.min.js");
const EPUBJS_JS: Asset = asset!("/assets/vendor/epub.min.js");
const READER_GLUE_JS: Asset = asset!("/assets/vendor/epub-reader-glue.js");

#[cfg(feature = "web")]
fn reader_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusReader && window.OmnibusReader.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

/// Full-screen EPUB reader page (web-feature interop, all-target chrome).
#[component]
pub fn BookReadPage(uuid: String) -> Element {
    let theme = use_context::<Signal<Theme>>();
    let ReaderSignals {
        status,
        show_aa,
        selection,
        highlights,
        toc,
        show_toc,
        show_highlights,
        note_target,
        quote_target,
        show_search,
        search_results,
        show_bookmarks,
        loc,
        book_meta,
    } = use_reader_signals(&uuid, theme);
    let (on_back, on_prev, on_next, on_keydown) = chrome_handlers::install_chrome_handlers(
        selection,
        chrome_handlers::OverlaySignals {
            show_aa,
            show_toc,
            show_search,
            show_highlights,
            show_bookmarks,
            note_target,
            quote_target,
        },
    );
    let ReaderDisplay {
        page_str,
        chapter_str,
        pct,
        chapter_title,
        current_cfi,
        book_title,
        book_author,
        book_accent,
    } = derive_reader_display(loc, book_meta);

    rsx! {
        ReaderLayout {
            meta: ReaderMeta {
                uuid: uuid.clone(),
                book_title,
                book_author,
                book_accent,
                chapter_title,
                current_cfi,
            },
            progress: ReaderProgress {
                page_str,
                chapter_str,
                pct,
                status: status(),
            },
            panels: ReaderPanelSignals {
                show_aa,
                selection,
                highlights,
                toc,
                show_toc,
                show_highlights,
                note_target,
                quote_target,
                show_search,
                search_results,
                show_bookmarks,
            },
            nav: ReaderNavHandlers {
                on_keydown,
                on_back,
                on_prev,
                on_next,
            },
        }
    }
}

/// Every signal `BookReadPage` owns, minus the reader-prefs context (which
/// is published but not otherwise threaded through the component body).
#[derive(Copy, Clone)]
struct ReaderSignals {
    status: Signal<ReaderStatus>,
    show_aa: Signal<bool>,
    selection: Signal<Option<SelectionData>>,
    highlights: Signal<Vec<Highlight>>,
    toc: Signal<Vec<TocEntry>>,
    show_toc: Signal<bool>,
    show_highlights: Signal<bool>,
    note_target: Signal<Option<Highlight>>,
    quote_target: Signal<Option<Highlight>>,
    show_search: Signal<bool>,
    search_results: Signal<Vec<SearchResult>>,
    show_bookmarks: Signal<bool>,
    loc: Signal<RelocateData>,
    book_meta: Signal<Option<omnibus_shared::EbookMetadata>>,
}

/// Construct every signal `BookReadPage` owns, publish `prefs` to context,
/// and (on web) install the epub.js JS interop. Must run unconditionally
/// from `BookReadPage` — every call here is a Dioxus hook, so the call
/// order has to stay stable across renders.
fn use_reader_signals(uuid: &str, theme: Signal<Theme>) -> ReaderSignals {
    // Seed identically on SSR and WASM so the first client render matches the
    // server-rendered markup — the overlay (rendered when `Loading`) must be
    // present in both, otherwise Dioxus mis-adopts nodes during hydration
    // (see .claude/rules/07-hydration.md). The web `use_effect` in
    // `install_reader_web_interop` transitions to `Ready` via the JS glue's
    // `on_status` callback after the reader mounts.
    let status = use_signal(ReaderStatus::default);

    let prefs = init_reader_prefs(theme);
    use_context_provider(|| prefs);

    let show_aa = use_signal(|| false);
    let selection: Signal<Option<SelectionData>> = use_signal(|| None);
    let highlights: Signal<Vec<Highlight>> = use_signal(Vec::new);
    let toc: Signal<Vec<TocEntry>> = use_signal(Vec::new);
    let show_toc = use_signal(|| false);
    let show_highlights = use_signal(|| false);
    let note_target: Signal<Option<Highlight>> = use_signal(|| None);
    let quote_target: Signal<Option<Highlight>> = use_signal(|| None);
    let show_search = use_signal(|| false);
    let search_results: Signal<Vec<SearchResult>> = use_signal(Vec::new);
    let show_bookmarks = use_signal(|| false);
    let loc = use_signal(RelocateData::default);
    let book_meta = use_book_metadata(uuid.to_string());

    #[cfg(feature = "web")]
    install_reader_web_interop(
        uuid.to_string(),
        prefs,
        InteropSignals {
            status,
            loc,
            selection,
            highlights,
            toc,
            search_results,
        },
    );

    // Suppress the `prefs` unused-variable warning on non-web targets:
    // `install_reader_web_interop` is web-only, so on SSR / mobile the
    // local binding is published via context but never read.
    #[cfg(not(feature = "web"))]
    let _ = prefs;

    ReaderSignals {
        status,
        show_aa,
        selection,
        highlights,
        toc,
        show_toc,
        show_highlights,
        note_target,
        quote_target,
        show_search,
        search_results,
        show_bookmarks,
        loc,
        book_meta,
    }
}

/// Display strings/values the reader chrome renders, derived from `loc` and `book_meta`.
struct ReaderDisplay {
    page_str: String,
    chapter_str: String,
    pct: u32,
    chapter_title: String,
    current_cfi: String,
    book_title: String,
    book_author: String,
    book_accent: String,
}

/// Derive page/chapter labels and book title/author/accent from `loc` and `book_meta`.
fn derive_reader_display(
    loc: Signal<RelocateData>,
    book_meta: Signal<Option<omnibus_shared::EbookMetadata>>,
) -> ReaderDisplay {
    let (page_str, chapter_str) = format_progress_labels(&loc.read());
    let pct = loc.read().pct;
    let chapter_title = loc.read().chapter_title.clone();
    let current_cfi = loc.read().cfi.clone().unwrap_or_default();
    let book_title = book_meta
        .read()
        .as_ref()
        .and_then(|b| b.title.clone())
        .unwrap_or_default();
    let book_author = book_meta
        .read()
        .as_ref()
        .and_then(|b| b.creators.first().map(|c| c.name.clone()))
        .unwrap_or_default();
    let book_accent = book_meta
        .read()
        .as_ref()
        .and_then(|b| b.accent.clone())
        .unwrap_or_else(|| "#3a3027".to_string());
    ReaderDisplay {
        page_str,
        chapter_str,
        pct,
        chapter_title,
        current_cfi,
        book_title,
        book_author,
        book_accent,
    }
}

/// Book identity + display strings the reader chrome renders.
#[derive(Clone, PartialEq)]
pub(super) struct ReaderMeta {
    pub uuid: String,
    pub book_title: String,
    pub book_author: String,
    pub book_accent: String,
    pub chapter_title: String,
    pub current_cfi: String,
}

/// Bottom-bar progress labels and the load-status overlay state.
#[derive(Clone, PartialEq)]
pub(super) struct ReaderProgress {
    pub page_str: String,
    pub chapter_str: String,
    pub pct: u32,
    pub status: ReaderStatus,
}

/// Overlay/panel signals the reader chrome toggles or renders from.
#[derive(Copy, Clone, PartialEq)]
pub(super) struct ReaderPanelSignals {
    pub show_aa: Signal<bool>,
    pub selection: Signal<Option<SelectionData>>,
    pub highlights: Signal<Vec<Highlight>>,
    pub toc: Signal<Vec<TocEntry>>,
    pub show_toc: Signal<bool>,
    pub show_highlights: Signal<bool>,
    pub note_target: Signal<Option<Highlight>>,
    pub quote_target: Signal<Option<Highlight>>,
    pub show_search: Signal<bool>,
    pub search_results: Signal<Vec<SearchResult>>,
    pub show_bookmarks: Signal<bool>,
}

/// Navigation + keyboard handlers for the top bar, page-turn gutters, and surface keydown.
#[derive(Copy, Clone, PartialEq)]
pub(super) struct ReaderNavHandlers {
    pub on_keydown: EventHandler<KeyboardEvent>,
    pub on_back: EventHandler<MouseEvent>,
    pub on_prev: EventHandler<MouseEvent>,
    pub on_next: EventHandler<MouseEvent>,
}

/// Reader chrome + panels (top bar, viewer stage, gutters, status bar, Aa panel, selection popover).
#[component]
fn ReaderLayout(
    meta: ReaderMeta,
    progress: ReaderProgress,
    panels: ReaderPanelSignals,
    nav: ReaderNavHandlers,
) -> Element {
    let ReaderMeta {
        uuid,
        book_title,
        book_author,
        book_accent,
        chapter_title,
        current_cfi,
    } = meta;
    let ReaderProgress {
        page_str,
        chapter_str,
        pct,
        status,
    } = progress;
    let ReaderNavHandlers {
        on_keydown,
        on_back,
        on_prev,
        on_next,
    } = nav;

    let show_aa = panels.show_aa;
    let selection = panels.selection;
    let highlights = panels.highlights;
    let note_target = panels.note_target;
    let quote_target = panels.quote_target;

    rsx! {
        document::Script { src: JSZIP_JS }
        document::Script { src: EPUBJS_JS }
        document::Script { src: READER_GLUE_JS }

        div {
            class: "rd-surface",
            tabindex: "0",
            autofocus: true,
            onkeydown: move |evt| on_keydown.call(evt),

            div { class: "rd-wash" }

            ReaderTopBar {
                book_title: book_title.clone(),
                chapter_title: chapter_title.clone(),
                panels,
                on_back,
            }

            ReaderViewerStage { status }

            ReaderPageTurnButtons { on_prev, on_next }

            div {
                class: "rd-bottom",
                span { style: "color:var(--ink-2);", "{page_str}" }
                div { style: "flex:1; text-align:center; letter-spacing:.08em;", "{chapter_str}" }
                span {}
            }
            div { class: "rd-ribbon", i { style: "width:{pct}%;" } }

            if show_aa() {
                ReaderAaPanel {
                    on_close: move |_| {
                        let mut show_aa = show_aa;
                        show_aa.set(false);
                    },
                }
            }

            ReaderSelectionPopover {
                uuid: uuid.clone(),
                selection,
                highlights,
                note_target,
                quote_target,
            }

            ReaderOverlays {
                meta: OverlayMeta {
                    uuid: uuid.clone(),
                    book_title: book_title.clone(),
                    book_author: book_author.clone(),
                    book_accent: book_accent.clone(),
                    chapter_title: chapter_title.clone(),
                    current_cfi: current_cfi.clone(),
                },
                panels,
            }
        }
    }
}

/// Selection popover: highlight / note / quote / copy / share actions for the
/// current text selection. Renders nothing when there is no selection.
#[component]
fn ReaderSelectionPopover(
    uuid: String,
    selection: Signal<Option<SelectionData>>,
    highlights: Signal<Vec<Highlight>>,
    note_target: Signal<Option<Highlight>>,
    quote_target: Signal<Option<Highlight>>,
) -> Element {
    let Some(sel) = selection.read().as_ref().cloned() else {
        return rsx! {};
    };
    rsx! {
        SelectionPopover {
            sel_rect_x: sel.rect.x,
            sel_rect_y: sel.rect.y,
            sel_rect_width: sel.rect.width,
            sel_cfi: sel.cfi_range.clone(),
            sel_text: sel.text.clone(),
            on_dismiss: move |_| {
                let mut selection = selection;
                selection.set(None);
            },
            on_highlight: {
                let uuid = uuid.clone();
                move |(cfi, color, text): (String, HighlightColor, String)| {
                    let mut selection = selection;
                    selection.set(None);
                    spawn_create_highlight(
                        uuid.clone(), cfi, color, text,
                        highlights, note_target, quote_target, PostCreate::None,
                    );
                }
            },
            on_note: {
                let uuid = uuid.clone();
                move |(cfi, text): (String, String)| {
                    let mut selection = selection;
                    selection.set(None);
                    spawn_create_highlight(
                        uuid.clone(), cfi, HighlightColor::Amber, text,
                        highlights, note_target, quote_target, PostCreate::Note,
                    );
                }
            },
            on_quote: {
                let uuid = uuid.clone();
                move |(cfi, text): (String, String)| {
                    let mut selection = selection;
                    selection.set(None);
                    spawn_create_highlight(
                        uuid.clone(), cfi, HighlightColor::Amber, text,
                        highlights, note_target, quote_target, PostCreate::Quote,
                    );
                }
            },
            on_copy: move |text: String| {
                #[cfg(feature = "web")]
                {
                    let lit = serde_json::to_string(&text)
                        .unwrap_or_else(|_| "\"\"".into());
                    reader_call("copyText", &lit);
                }
                let _ = &text;
                let mut selection = selection;
                selection.set(None);
            },
            on_share: move |text: String| {
                #[cfg(feature = "web")]
                {
                    let lit = serde_json::to_string(&text)
                        .unwrap_or_else(|_| "\"\"".into());
                    reader_call("shareText", &lit);
                }
                let _ = &text;
                let mut selection = selection;
                selection.set(None);
            },
        }
    }
}

/// Book identity + display strings the overlay drawers render.
#[derive(Clone, PartialEq)]
pub(super) struct OverlayMeta {
    pub uuid: String,
    pub book_title: String,
    pub book_author: String,
    pub book_accent: String,
    pub chapter_title: String,
    pub current_cfi: String,
}

/// The toggleable reader overlays: contents, highlights, search, bookmarks
/// drawers plus the quote panel and note composer. Each renders only when its
/// backing signal is set.
#[component]
fn ReaderOverlays(meta: OverlayMeta, panels: ReaderPanelSignals) -> Element {
    let OverlayMeta {
        uuid,
        book_title,
        book_author,
        book_accent,
        chapter_title,
        current_cfi,
    } = meta;
    let ReaderPanelSignals {
        highlights,
        toc,
        show_toc,
        show_highlights,
        note_target,
        quote_target,
        show_search,
        search_results,
        show_bookmarks,
        ..
    } = panels;

    rsx! {
        if show_toc() {
            TocDrawer {
                entries: toc.read().clone(),
                current_title: chapter_title.clone(),
                on_navigate: move |href: String| {
                    #[cfg(feature = "web")]
                    {
                        let lit = serde_json::to_string(&href).unwrap_or_else(|_| "\"\"".into());
                        reader_call("display", &lit);
                    }
                    let _ = &href;
                    let mut show_toc = show_toc;
                    show_toc.set(false);
                },
                on_close: move |_| {
                    let mut show_toc = show_toc;
                    show_toc.set(false);
                },
            }
        }

        if show_highlights() {
            HighlightsDrawer {
                highlights,
                on_quote: move |h: Highlight| {
                    let mut quote_target = quote_target;
                    let mut show_highlights = show_highlights;
                    quote_target.set(Some(h));
                    show_highlights.set(false);
                },
                on_edit_note: move |h: Highlight| {
                    let mut note_target = note_target;
                    let mut show_highlights = show_highlights;
                    note_target.set(Some(h));
                    show_highlights.set(false);
                },
                on_close: move |_| {
                    let mut show_highlights = show_highlights;
                    show_highlights.set(false);
                },
            }
        }

        if show_search() {
            SearchPanel {
                results: search_results,
                on_query: move |q: String| {
                    #[cfg(feature = "web")]
                    {
                        let lit = serde_json::to_string(&q).unwrap_or_else(|_| "\"\"".into());
                        reader_call("search", &lit);
                    }
                    let _ = &q;
                },
                on_navigate: move |cfi: String| {
                    #[cfg(feature = "web")]
                    {
                        let lit = serde_json::to_string(&cfi).unwrap_or_else(|_| "\"\"".into());
                        reader_call("display", &lit);
                    }
                    let _ = &cfi;
                    let mut show_search = show_search;
                    show_search.set(false);
                },
                on_close: move |_| {
                    let mut show_search = show_search;
                    show_search.set(false);
                },
            }
        }

        if show_bookmarks() {
            ReaderBookmarksDrawer {
                uuid: uuid.clone(),
                current_cfi: current_cfi.clone(),
                current_label: chapter_title.clone(),
                on_navigate: move |cfi: String| {
                    #[cfg(feature = "web")]
                    {
                        let lit = serde_json::to_string(&cfi).unwrap_or_else(|_| "\"\"".into());
                        reader_call("display", &lit);
                    }
                    let _ = &cfi;
                    let mut show_bookmarks = show_bookmarks;
                    show_bookmarks.set(false);
                },
                on_close: move |_| {
                    let mut show_bookmarks = show_bookmarks;
                    show_bookmarks.set(false);
                },
            }
        }

        if let Some(h) = quote_target.read().clone() {
            QuotePanel {
                quote_text: h.text.clone().unwrap_or_default(),
                author: book_author.clone(),
                subtitle: book_title.clone(),
                accent: book_accent.clone(),
                on_close: move |_| {
                    let mut quote_target = quote_target;
                    quote_target.set(None);
                },
            }
        }

        if let Some(h) = note_target.read().clone() {
            NoteComposer {
                highlight: h,
                on_saved: move |(id, note): (i64, Option<String>)| {
                    let mut highlights = highlights;
                    let idx = highlights.read().iter().position(|x| x.id == id);
                    if let Some(i) = idx {
                        highlights.write()[i].note = note;
                    }
                },
                on_close: move |_| {
                    let mut note_target = note_target;
                    note_target.set(None);
                },
            }
        }
    }
}

/// Owns the mutually-exclusive overlay toggles and renders [`ReaderTopChrome`].
/// Each tool closes every other surface before opening its own.
#[component]
fn ReaderTopBar(
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
            book_title,
            chapter_title,
            show_aa: show_aa(),
            toc_active: show_toc(),
            search_active: show_search(),
            highlights_active: show_highlights(),
            bookmarks_active: show_bookmarks(),
            highlight_count,
            on_back,
            on_toggle_aa: move |_| {
                let cur = show_aa();
                close_overlays();
                let mut s = show_aa;
                s.set(!cur);
            },
            on_toggle_toc: move |_| {
                let cur = show_toc();
                close_overlays();
                let mut s = show_toc;
                s.set(!cur);
            },
            on_toggle_search: move |_| {
                let cur = show_search();
                close_overlays();
                let mut s = show_search;
                s.set(!cur);
            },
            on_toggle_highlights: move |_| {
                let cur = show_highlights();
                close_overlays();
                let mut s = show_highlights;
                s.set(!cur);
            },
            on_toggle_bookmarks: move |_| {
                let cur = show_bookmarks();
                close_overlays();
                let mut s = show_bookmarks;
                s.set(!cur);
            },
        }
    }
}

/// Top navigation bar: back button, title + chapter display, Aa + bookmark tools.
#[component]
fn ReaderTopChrome(
    book_title: String,
    chapter_title: String,
    show_aa: bool,
    toc_active: bool,
    search_active: bool,
    highlights_active: bool,
    bookmarks_active: bool,
    highlight_count: usize,
    on_back: EventHandler<MouseEvent>,
    on_toggle_aa: EventHandler<MouseEvent>,
    on_toggle_toc: EventHandler<MouseEvent>,
    on_toggle_search: EventHandler<MouseEvent>,
    on_toggle_highlights: EventHandler<MouseEvent>,
    on_toggle_bookmarks: EventHandler<MouseEvent>,
) -> Element {
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
fn ReaderViewerStage(status: ReaderStatus) -> Element {
    rsx! {
        div {
            class: "rd-stage",
            style: "top:60px; bottom:54px;",
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
fn ReaderPageTurnButtons(
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
