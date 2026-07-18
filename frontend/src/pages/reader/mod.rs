//! Immersive full-screen EPUB reader. Loads the vendored epub.js +
//! JSZip glue (`window.OmnibusReader`) via `dioxus::document::eval`, streams
//! bytes from `GET /api/ebooks/:uuid/file`, and persists position via
//! [`crate::reader_progress`]. Chrome compiles on every target; the JS interop
//! that mounts a book has a per-target seam — `interop` (web, `wasm_bindgen`
//! callbacks + same-origin cookie fetch) and `mobile` (wry WebView,
//! `dioxus.send` event channel + tokened cross-origin fetch).

mod aa_panel;
mod annotations_sheet;
mod bootstrap;
mod chrome;
mod chrome_handlers;
mod highlights;
mod highlights_drawer;
#[cfg(feature = "web")]
mod interop;
#[cfg(feature = "mobile")]
mod mobile;
mod note_composer;
mod overlays;
mod prefs;
mod quote_panel;
mod reader_bookmarks;
mod search_panel;
mod selection;
mod signals;
mod toc_drawer;
mod typography;

use dioxus::prelude::*;
use omnibus_shared::Highlight;

use crate::components::atrium::Theme;

use aa_panel::ReaderAaPanel;
use chrome::{ReaderPageTurnButtons, ReaderTopBar, ReaderViewerStage};
#[cfg(feature = "web")]
use interop::{install_reader_web_interop, InteropSignals};
use overlays::{OverlayMeta, ReaderOverlays, ReaderSelectionPopover};
use prefs::init_reader_prefs;
use search_panel::SearchResult;
use selection::SelectionData;
use signals::{
    format_contents_progress, format_progress_labels, format_title_sub, use_book_metadata,
    ReaderStatus, RelocateData,
};
use toc_drawer::TocEntry;

const JSZIP_JS: Asset = asset!("/assets/vendor/jszip.min.js");
const EPUBJS_JS: Asset = asset!("/assets/vendor/epub.min.js");
const READER_GLUE_JS: Asset = asset!("/assets/vendor/epub-reader-glue.js");

// The `reader_call*` helpers drive the same `window.OmnibusReader` glue on both
// interactive targets: web (WASM) and mobile (wry WebView). Only SSR compiles
// them out. `dioxus::document::eval` is the shared seam.
#[cfg(any(feature = "web", feature = "mobile"))]
fn reader_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusReader && window.OmnibusReader.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

/// Encode `value` as a JS literal, falling back to `null` on the
/// unreachable-in-practice encode failure (every caller passes a
/// `str`/enum/bool, none of which can fail to serialize).
#[cfg(any(feature = "web", feature = "mobile"))]
fn json_literal<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// [`reader_call`] with a single JSON-encoded argument — the common case.
#[cfg(any(feature = "web", feature = "mobile"))]
fn reader_call_json<T: serde::Serialize + ?Sized>(method: &str, value: &T) {
    reader_call(method, &json_literal(value));
}

/// [`reader_call`] with two JSON-encoded arguments, e.g.
/// `addAnnotation(cfi, color)`.
#[cfg(any(feature = "web", feature = "mobile"))]
fn reader_call_json2<A: serde::Serialize + ?Sized, B: serde::Serialize + ?Sized>(
    method: &str,
    a: &A,
    b: &B,
) {
    reader_call(method, &format!("{}, {}", json_literal(a), json_literal(b)));
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
        show_annotations,
        loc,
        book_meta,
        chrome_hidden,
    } = use_reader_signals(&uuid, theme);
    let (on_back, on_prev, on_next, on_keydown) = chrome_handlers::install_chrome_handlers(
        uuid.clone(),
        selection,
        chrome_handlers::OverlaySignals {
            show_aa,
            show_toc,
            show_search,
            show_highlights,
            show_bookmarks,
            show_annotations,
            note_target,
            quote_target,
        },
    );
    let ReaderDisplay {
        page_str,
        chapter_str,
        title_sub,
        contents_progress,
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
                contents_progress,
            },
            progress: ReaderProgress {
                page_str,
                chapter_str,
                title_sub,
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
                show_annotations,
            },
            nav: ReaderNavHandlers {
                on_keydown,
                on_back,
                on_prev,
                on_next,
            },
            chrome_hidden,
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
    show_annotations: Signal<bool>,
    loc: Signal<RelocateData>,
    book_meta: Signal<Option<omnibus_shared::EbookMetadata>>,
    chrome_hidden: Signal<bool>,
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
    let show_annotations = use_signal(|| false);
    let loc = use_signal(RelocateData::default);
    let book_meta = use_book_metadata(uuid.to_string());
    // Chrome starts visible; a centre tap (glue → `__omnibusOnToggleChrome`)
    // flips this. Seeded identically so SSR and the first WASM paint match,
    // keeping hydration stable — the class only appears after a post-mount tap.
    let chrome_hidden = use_signal(|| false);

    // Record reading time against this book while the reader is open (and,
    // on web, the tab visible) — the rows behind the `/stats` aggregates.
    // Effect-only (no rsx), so SSR is a no-op and hydration is unaffected.
    crate::session_tracker::use_reading_session(
        uuid.to_string(),
        crate::contexts::use_server_url(),
    );

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
            chrome_hidden,
        },
    );

    // Mobile drives the same glue through the wry WebView's `dioxus.send`
    // event channel rather than `wasm_bindgen` window callbacks. `use_effect`
    // hooks inside run unconditionally, so hook order stays stable (rule 07).
    #[cfg(feature = "mobile")]
    mobile::install_reader_mobile_interop(
        uuid.to_string(),
        prefs,
        mobile::InteropSignals {
            status,
            loc,
            selection,
            highlights,
            toc,
            search_results,
            chrome_hidden,
        },
        crate::contexts::use_server_url(),
    );

    // Suppress the `prefs` unused-variable warning on SSR: the interop that
    // reads it is web/mobile-only, so on SSR the binding is published via
    // context but never read.
    #[cfg(not(any(feature = "web", feature = "mobile")))]
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
        show_annotations,
        loc,
        book_meta,
        chrome_hidden,
    }
}

/// Display strings/values the reader chrome renders, derived from `loc` and `book_meta`.
struct ReaderDisplay {
    page_str: String,
    chapter_str: String,
    title_sub: String,
    contents_progress: String,
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
    // One read: every derived label comes from the same relocate snapshot.
    let loc_now = loc.read();
    let (page_str, chapter_str) = format_progress_labels(&loc_now);
    let title_sub = format_title_sub(&loc_now);
    let contents_progress = format_contents_progress(&loc_now);
    let pct = loc_now.pct;
    let chapter_title = loc_now.chapter_title.clone();
    let current_cfi = loc_now.cfi.clone().unwrap_or_default();
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
        title_sub,
        contents_progress,
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
    /// Contents-drawer progress line ("184 / 272 · 68%").
    pub contents_progress: String,
}

/// Bottom-bar progress labels and the load-status overlay state.
#[derive(Clone, PartialEq)]
pub(super) struct ReaderProgress {
    pub page_str: String,
    pub chapter_str: String,
    /// Phone top-bar sub-line ("Ch. 14 · 68%") — rendered on every target,
    /// shown only by the phone breakpoint.
    pub title_sub: String,
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
    pub show_annotations: Signal<bool>,
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
    chrome_hidden: Signal<bool>,
) -> Element {
    let ReaderMeta {
        uuid,
        book_title,
        book_author,
        book_accent,
        chapter_title,
        current_cfi,
        contents_progress,
    } = meta;
    let ReaderProgress {
        page_str,
        chapter_str,
        title_sub,
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

    // Web/SSR emit ordered, parser-inserted tags (JSZip before epub.js, which
    // binds `window.JSZip` at load time). Mobile has no SSR, so it loads the
    // runtime in order from `mobile::interop` instead (see `install_surface_js`).
    #[cfg(not(feature = "mobile"))]
    let reader_scripts = rsx! {
        document::Script { src: JSZIP_JS }
        document::Script { src: EPUBJS_JS }
        document::Script { src: READER_GLUE_JS }
    };
    #[cfg(feature = "mobile")]
    let reader_scripts = rsx! {};

    rsx! {
        {reader_scripts}

        div {
            class: if chrome_hidden() { "rd-surface rd-chrome-hidden" } else { "rd-surface" },
            tabindex: "0",
            autofocus: true,
            onkeydown: move |evt| on_keydown.call(evt),

            div { class: "rd-wash" }

            ReaderTopBar {
                book_title: book_title.clone(),
                chapter_title: chapter_title.clone(),
                title_sub: title_sub.clone(),
                panels,
                on_back,
            }

            ReaderViewerStage { status }

            ReaderPageTurnButtons { on_prev, on_next }

            div {
                class: "rd-bottom",
                "data-testid": "reader-footer",
                span { style: "color:var(--ink-2);", "{page_str}" }
                div { style: "flex:1; text-align:center; letter-spacing:.08em;", "{chapter_str}" }
                // The phone footer moves the chapter position to the right
                // edge (the centred div above is hidden there) — rendered on
                // every target, shown only by the phone breakpoint (rule 07).
                span { class: "rd-bottom-ch", "{chapter_str}" }
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
                    contents_progress: contents_progress.clone(),
                },
                panels,
            }
        }
    }
}
