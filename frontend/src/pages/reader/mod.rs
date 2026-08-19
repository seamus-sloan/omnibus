//! Immersive full-screen EPUB reader. Loads the vendored epub.js + JSZip glue
//! (`window.OmnibusReader`) via `dioxus::document::eval`, streams bytes from
//! `GET /api/ebooks/:uuid/file`, and persists position via
//! [`crate::reader_progress`]. Chrome compiles on every target; the JS interop
//! that mounts a book has a per-target seam — `interop` (web) and `mobile`.

mod aa_panel;
mod annotations_sheet;
mod bootstrap;
mod chrome;
mod chrome_handlers;
mod drawer_shell;
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
mod sync_banner;
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
    format_ambient_page, format_contents_progress, format_progress_labels, format_title_sub,
    use_book_metadata, ReaderStatus, RelocateData,
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

/// [`reader_call`] with a single JSON-encoded argument — the common case.
/// JSON-literal encoding lives in [`crate::js_interop`], shared with the
/// other JS-bridge modules (barcode scanner, mobile audio).
#[cfg(any(feature = "web", feature = "mobile"))]
fn reader_call_json<T: serde::Serialize + ?Sized>(method: &str, value: &T) {
    reader_call(method, &crate::js_interop::json_literal(value));
}

/// [`reader_call`] with two JSON-encoded arguments, e.g.
/// `addAnnotation(cfi, color)`.
#[cfg(any(feature = "web", feature = "mobile"))]
fn reader_call_json2<A: serde::Serialize + ?Sized, B: serde::Serialize + ?Sized>(
    method: &str,
    a: &A,
    b: &B,
) {
    reader_call(
        method,
        &format!(
            "{}, {}",
            crate::js_interop::json_literal(a),
            crate::js_interop::json_literal(b)
        ),
    );
}

/// Full-screen EPUB reader page (web-feature interop, all-target chrome).
#[component]
pub fn BookReadPage(uuid: String) -> Element {
    let theme = use_context::<Signal<Theme>>();
    use_mobile_offline_read_guard(uuid.clone());
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
    let chrome_handlers::ChromeHandlers {
        on_back,
        on_prev,
        on_next,
        on_retry,
        on_keydown,
    } = chrome_handlers::install_chrome_handlers(
        uuid.clone(),
        selection,
        status,
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
        ambient_page,
        title_sub,
        contents_progress,
        pct,
        chapter_title,
        current_cfi,
        book_title,
        book_author,
        book_accent,
    } = derive_reader_display(loc, book_meta, status());

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
                ambient_page,
                title_sub,
                pct,
                status: status(),
                loc,
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
                status,
            },
            nav: ReaderNavHandlers {
                on_keydown,
                on_back,
                on_prev,
                on_next,
                on_retry,
            },
            chrome_hidden,
        }
    }
}

/// Mobile: bounce out of the reader route (raising the app-level offline
/// sheet) when the book has no completed local EPUB download and the app is
/// known-offline — mounting would strand the user on a surface that can
/// never load. `go_back` returns to wherever the tap came from; a cold
/// deep-link with no history falls back to the book page.
#[cfg(feature = "mobile")]
fn use_mobile_offline_read_guard(uuid: String) {
    let nav = dioxus_router::use_navigator();
    use_effect(use_reactive!(|uuid| {
        if crate::offline::sync::is_offline()
            && crate::offline::downloads::local_epub_url(&uuid).is_none()
        {
            crate::components::offline_guard::block(
                "This book isn\u{2019}t downloaded, so it can\u{2019}t be read while offline.",
            );
            if nav.can_go_back() {
                nav.go_back();
            } else {
                nav.replace(crate::Route::BookDetail { uuid: uuid.clone() });
            }
        }
    }));
}

/// Non-mobile stub: web/SSR have no offline layer.
#[cfg(not(feature = "mobile"))]
fn use_mobile_offline_read_guard(_uuid: String) {}

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
    // Chrome starts hidden on the native app (Books-style: opening a book
    // lands on the clean page; a centre tap summons the menus) and visible
    // everywhere else. `cfg!` is a compile-time constant, so SSR and the web
    // client both seed `false` — first-paint markup still matches and
    // hydration stays stable (rule 07); mobile has no SSR to mismatch.
    let chrome_hidden = use_signal(|| cfg!(feature = "mobile"));

    // Record reading time against this book while the reader is open (and,
    // on web, the tab visible) — the rows behind the `/stats` aggregates.
    // Effect-only (no rsx), so SSR is a no-op and hydration is unaffected.
    crate::session_tracker::use_reading_session(
        uuid.to_string(),
        crate::contexts::use_server_url(),
    );

    // Auto read-status, plus the stale-relocate reset on a same-component
    // book swap. Effect-only, like the session tracker above.
    use_auto_read_status_for_reader(uuid, loc);

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

/// Auto read-status wiring for the reader: opening an `Unread` book marks it
/// `Reading`; the first relocate whose range reaches the book's end marks it
/// `Finished` (never a downgrade). Also resets `loc` on a same-component book
/// swap (SPA nav between `/read/:uuid` routes) — otherwise a stale `at_end`
/// from the previous book could mark the next one finished before its first
/// relocate arrives. Split out of [`use_reader_signals`] as its own hook
/// (call-order-stable, so it's still safe to invoke unconditionally from the
/// component body) — mirrors `use_retarget_playback`/
/// `use_marquee_title_refresh` in `pages/listen/mobile/effects.rs`.
fn use_auto_read_status_for_reader(uuid: &str, loc: Signal<RelocateData>) {
    let at_end = use_memo(move || loc.read().at_end);
    crate::read_status_auto::use_auto_read_status(
        uuid.to_string(),
        crate::contexts::use_server_url(),
        at_end,
    );

    let mut loc = loc;
    let uuid_dep = uuid.to_string();
    use_effect(use_reactive!(|uuid_dep| {
        let _ = &uuid_dep;
        loc.set(RelocateData::default());
    }));
}

/// Display strings/values the reader chrome renders, derived from `loc` and `book_meta`.
struct ReaderDisplay {
    page_str: String,
    chapter_str: String,
    ambient_page: String,
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
///
/// `status` gates the top-bar chapter readout: while a TOC/bookmark/search
/// jump is in flight (`ReaderStatus::Loading`), the header blanks the
/// previous chapter's title/sub-line rather than showing it alongside the
/// loading affordance — see [`ReaderStatus`] and issue #1909 (AC3). The
/// footer/ribbon strings are left alone; they settle the moment the new
/// relocate lands, same as `status` itself.
fn derive_reader_display(
    loc: Signal<RelocateData>,
    book_meta: Signal<Option<omnibus_shared::EbookMetadata>>,
    status: ReaderStatus,
) -> ReaderDisplay {
    // One read: every derived label comes from the same relocate snapshot.
    let loc_now = loc.read();
    let (page_str, chapter_str) = format_progress_labels(&loc_now);
    let ambient_page = format_ambient_page(&loc_now);
    let loading = status == ReaderStatus::Loading;
    let title_sub = if loading {
        String::new()
    } else {
        format_title_sub(&loc_now)
    };
    let contents_progress = format_contents_progress(&loc_now);
    let pct = loc_now.pct;
    let chapter_title = if loading {
        String::new()
    } else {
        loc_now.chapter_title.clone()
    };
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
        ambient_page,
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
    /// Phone minimal-chrome footer: the bare page number ("142"). Rendered
    /// on every target, shown only by the phone breakpoint while the
    /// chrome is hidden (the richer labels swap in with the chrome).
    pub ambient_page: String,
    /// Phone top-bar sub-line ("Ch. 14 · 68%") — rendered on every target,
    /// shown only by the phone breakpoint.
    pub title_sub: String,
    pub pct: u32,
    /// The live relocation signal itself — the "synced here" pill reads
    /// the full-precision fraction at click time.
    pub loc: Signal<RelocateData>,
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
    /// Threaded through so a TOC jump can flip the reader into its loading
    /// state before asking the glue to navigate (see `overlays::render_toc_overlay`).
    pub status: Signal<ReaderStatus>,
}

/// Navigation + keyboard handlers for the top bar, page-turn gutters, and surface keydown.
#[derive(Copy, Clone, PartialEq)]
pub(super) struct ReaderNavHandlers {
    pub on_keydown: EventHandler<KeyboardEvent>,
    pub on_back: EventHandler<MouseEvent>,
    pub on_prev: EventHandler<MouseEvent>,
    pub on_next: EventHandler<MouseEvent>,
    /// The error overlay's "Retry" action (issue #1895, AC3).
    pub on_retry: EventHandler<MouseEvent>,
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
        ambient_page,
        title_sub,
        pct,
        status,
        loc,
    } = progress;
    let ReaderNavHandlers {
        on_keydown,
        on_back,
        on_prev,
        on_next,
        on_retry,
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
        document::Script { src: crate::components::quote_card::QUOTE_CARD_JS }
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

            // Ambient minimal-chrome label: the book title that fades in where
            // the toolbar was while the chrome is hidden (phone breakpoint
            // only; displayed purely by CSS so every target renders it — rule 07).
            // aria-hidden: it only mirrors the toolbar title visually, so it
            // must never be announced alongside it.
            div { class: "rd-ambient-title", "aria-hidden": "true", "{book_title}" }

            ReaderTopBar {
                book_title: book_title.clone(),
                chapter_title: chapter_title.clone(),
                title_sub: title_sub.clone(),
                panels,
                on_back,
            }

            {sync_banner_slot(&uuid)}

            ReaderViewerStage { status, on_retry }

            ReaderPageTurnButtons { on_prev, on_next }

            div {
                class: "rd-bottom",
                "data-testid": "reader-footer",
                span { class: "rd-bottom-page", style: "color:var(--ink-2);", "{page_str}" }
                {sync_here_slot(&uuid, loc)}
                div { style: "flex:1; text-align:center; letter-spacing:.08em;", "{chapter_str}" }
                // The phone footer moves the chapter position to the right
                // edge (the centred div above is hidden there) — rendered on
                // every target, shown only by the phone breakpoint (rule 07).
                span { class: "rd-bottom-ch", "{chapter_str}" }
                // Phone minimal-chrome swap: the bare centred page number the
                // footer shows while the chrome is hidden (CSS-only display,
                // rendered on every target — rule 07).
                span { class: "rd-ambient-page", "{ambient_page}" }
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

/// Web-only slot for the cross-format jump banner; the hybrid mobile
/// shell renders nothing here.
#[cfg(not(feature = "mobile"))]
fn sync_banner_slot(uuid: &str) -> Element {
    rsx! {
        sync_banner::SyncJumpBanner { uuid: uuid.to_string() }
    }
}

#[cfg(feature = "mobile")]
fn sync_banner_slot(_uuid: &str) -> Element {
    rsx! {}
}

/// Web-only slot for the "synced here" footer pill (same split as
/// [`sync_banner_slot`] — native iOS owns the gesture on mobile).
#[cfg(not(feature = "mobile"))]
fn sync_here_slot(uuid: &str, loc: Signal<RelocateData>) -> Element {
    rsx! {
        sync_banner::SyncHerePill { uuid: uuid.to_string(), loc }
    }
}

#[cfg(feature = "mobile")]
fn sync_here_slot(_uuid: &str, _loc: Signal<RelocateData>) -> Element {
    rsx! {}
}

#[cfg(test)]
mod tests;
