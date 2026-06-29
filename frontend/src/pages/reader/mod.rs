//! Immersive full-screen EPUB reader. Loads the vendored epub.js +
//! JSZip glue (`window.OmnibusReader`) via `dioxus::document::eval`, streams
//! bytes from cookie-gated `GET /api/ebooks/:uuid/file`, and persists
//! position via [`crate::reader_progress`]. Chrome compiles on every
//! target; the JS interop that mounts a book is web-only.

mod aa_panel;
mod bootstrap;
mod chrome_handlers;
mod highlights_drawer;
#[cfg(feature = "web")]
mod interop;
mod note_composer;
mod prefs;
mod selection;
mod toc_drawer;
mod typography;

use dioxus::prelude::*;

use crate::components::atrium::Theme;
use crate::contexts::use_server_url;
use crate::data;

use omnibus_shared::{EbookMetadata, Highlight, HighlightColor};

use aa_panel::ReaderAaPanel;
use highlights_drawer::HighlightsDrawer;
#[cfg(feature = "web")]
use interop::{install_reader_web_interop, InteropSignals};
use note_composer::NoteComposer;
use prefs::init_reader_prefs;
use selection::{SelectionData, SelectionPopover};
use toc_drawer::{TocDrawer, TocEntry};

const JSZIP_JS: Asset = asset!("/assets/vendor/jszip.min.js");
const EPUBJS_JS: Asset = asset!("/assets/vendor/epub.min.js");
const READER_GLUE_JS: Asset = asset!("/assets/vendor/epub-reader-glue.js");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
enum ReaderStatus {
    // INVARIANT: `Loading` is the SSR/WASM-first-paint seed (see
    // `BookReadPage`). Changing the default flips the rendered overlay and
    // breaks hydration — see .claude/rules/07-hydration.md.
    #[default]
    Loading,
    Ready,
    Failed,
}

/// Relocated event data from epub.js glue (deserialized from JSON).
#[derive(Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelocateData {
    // Only the web-feature progress-save path reads `cfi`; the other fields are read unconditionally by the bottom-bar render.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    cfi: Option<String>,
    page: u32,
    total_pages: u32,
    pct: u32,
    chapter: u32,
    total_chapters: u32,
    chapter_title: String,
}

#[cfg(feature = "web")]
fn reader_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusReader && window.OmnibusReader.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

/// Optimistically annotate the selection, persist the highlight, and — when
/// `open_note` — open the note composer on the created row. Shared by the
/// swatch (highlight) and Note popover actions. Rolls the optimistic
/// annotation back if the write fails.
fn spawn_create_highlight(
    uuid: String,
    cfi: String,
    color: HighlightColor,
    text: String,
    mut highlights: Signal<Vec<Highlight>>,
    mut note_target: Signal<Option<Highlight>>,
    open_note: bool,
) {
    #[cfg(feature = "web")]
    {
        let cfi_lit = serde_json::to_string(&cfi).unwrap_or_else(|_| "\"\"".into());
        let color_lit =
            serde_json::to_string(color.as_str()).unwrap_or_else(|_| "\"amber\"".into());
        reader_call("addAnnotation", &format!("{cfi_lit}, {color_lit}"));
    }
    let create = omnibus_shared::CreateHighlight {
        book_uuid: uuid,
        epub_cfi_range: cfi.clone(),
        color,
        text: if text.is_empty() { None } else { Some(text) },
    };
    spawn(async move {
        match data::create_highlight("", create).await {
            Ok(h) => {
                highlights.write().push(h.clone());
                if open_note {
                    note_target.set(Some(h));
                }
            }
            Err(_) => {
                #[cfg(feature = "web")]
                {
                    let cfi_lit = serde_json::to_string(&cfi).unwrap_or_else(|_| "\"\"".into());
                    reader_call("removeAnnotation", &cfi_lit);
                }
                let _ = &cfi;
            }
        }
    });
}

/// Extract `file_id` from the current URL's query string (`?file_id=N`),
/// targeting a specific `book_files` row for multi-file books.
#[cfg(feature = "web")]
fn parse_file_id_from_url() -> Option<i64> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("file_id")?.parse().ok()
}

/// Format the bottom-bar `page` and `chapter` strings from a relocate
/// event. Returns `("", "")` until epub.js has produced a relocation.
fn format_progress_labels(loc: &RelocateData) -> (String, String) {
    let page = if loc.total_pages > 0 {
        format!(
            "p.\u{a0}{} of {}\u{a0}\u{b7}\u{a0}{}%",
            loc.page, loc.total_pages, loc.pct
        )
    } else if loc.pct > 0 {
        format!("{}%", loc.pct)
    } else {
        String::new()
    };
    let chapter = if loc.total_chapters > 0 {
        format!("Ch\u{a0}{} of {}", loc.chapter, loc.total_chapters)
    } else {
        String::new()
    };
    (page, chapter)
}

/// Drop the previous book's title and kick off a fresh `get_ebook` fetch
/// whenever `uuid` changes. SPA navigations between books would otherwise
/// flash the previous title while the request is in flight.
fn use_book_metadata(uuid: String) -> Signal<Option<EbookMetadata>> {
    let mut book_meta: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let server_url = use_server_url();
    use_effect(use_reactive!(|uuid| {
        book_meta.set(None);
        let url = server_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Ok(Some(b)) = data::get_ebook(&url, &uuid).await {
                book_meta.set(Some(b));
            }
        });
    }));
    book_meta
}

/// Full-screen EPUB reader page (web-feature interop, all-target chrome).
#[component]
pub fn BookReadPage(uuid: String) -> Element {
    let theme = use_context::<Signal<Theme>>();

    // Seed identically on SSR and WASM so the first client render matches the
    // server-rendered markup — the overlay (rendered when `Loading`) must be
    // present in both, otherwise Dioxus mis-adopts nodes during hydration
    // (see .claude/rules/07-hydration.md). The web `use_effect` below
    // transitions to `Ready` via the JS glue's `on_status` callback after the
    // reader mounts.
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
    let loc = use_signal(RelocateData::default);
    let book_meta = use_book_metadata(uuid.clone());

    #[cfg(feature = "web")]
    install_reader_web_interop(
        uuid.clone(),
        prefs,
        InteropSignals {
            status,
            loc,
            selection,
            highlights,
            toc,
        },
    );

    let (on_back, on_prev, on_next, on_keydown) = chrome_handlers::install_chrome_handlers(
        selection,
        chrome_handlers::OverlaySignals {
            show_aa,
            show_toc,
            show_highlights,
            note_target,
        },
    );

    let (page_str, chapter_str) = format_progress_labels(&loc.read());
    let pct = loc.read().pct;
    let chapter_title_display = loc.read().chapter_title.clone();
    let book_title = book_meta
        .read()
        .as_ref()
        .and_then(|b| b.title.clone())
        .unwrap_or_default();

    // Suppress the `prefs` unused-variable warning on non-web targets:
    // `install_reader_web_interop` is web-only, so on SSR / mobile the
    // local binding is published via context but never read.
    #[cfg(not(feature = "web"))]
    let _ = prefs;

    rsx! {
        ReaderLayout {
            uuid: uuid.clone(),
            book_title,
            chapter_title: chapter_title_display,
            page_str,
            chapter_str,
            pct,
            status: status(),
            show_aa,
            selection,
            highlights,
            toc,
            show_toc,
            show_highlights,
            note_target,
            on_keydown,
            on_back,
            on_prev,
            on_next,
        }
    }
}

/// Reader chrome + panels (top bar, viewer stage, gutters, status bar, Aa panel, selection popover).
#[component]
fn ReaderLayout(
    uuid: String,
    book_title: String,
    chapter_title: String,
    page_str: String,
    chapter_str: String,
    pct: u32,
    status: ReaderStatus,
    show_aa: Signal<bool>,
    selection: Signal<Option<SelectionData>>,
    highlights: Signal<Vec<Highlight>>,
    toc: Signal<Vec<TocEntry>>,
    show_toc: Signal<bool>,
    show_highlights: Signal<bool>,
    note_target: Signal<Option<Highlight>>,
    on_keydown: EventHandler<KeyboardEvent>,
    on_back: EventHandler<MouseEvent>,
    on_prev: EventHandler<MouseEvent>,
    on_next: EventHandler<MouseEvent>,
) -> Element {
    let highlight_count = highlights.read().len();
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

            ReaderTopChrome {
                book_title: book_title.clone(),
                chapter_title: chapter_title.clone(),
                show_aa: show_aa(),
                toc_active: show_toc(),
                highlights_active: show_highlights(),
                highlight_count,
                on_back,
                on_toggle_aa: move |_| {
                    let mut show_aa = show_aa;
                    show_aa.set(!show_aa());
                },
                on_toggle_toc: move |_| {
                    let mut show_toc = show_toc;
                    let mut show_highlights = show_highlights;
                    let next = !show_toc();
                    show_toc.set(next);
                    if next {
                        show_highlights.set(false);
                    }
                },
                on_toggle_highlights: move |_| {
                    let mut show_toc = show_toc;
                    let mut show_highlights = show_highlights;
                    let next = !show_highlights();
                    show_highlights.set(next);
                    if next {
                        show_toc.set(false);
                    }
                },
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

            if let Some(sel) = selection.read().as_ref() {
                {
                    let uuid_pop = uuid.clone();
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
                                let uuid_pop = uuid_pop.clone();
                                move |(cfi, color, text): (String, HighlightColor, String)| {
                                    let mut selection = selection;
                                    selection.set(None);
                                    spawn_create_highlight(
                                        uuid_pop.clone(), cfi, color, text,
                                        highlights, note_target, false,
                                    );
                                }
                            },
                            on_note: {
                                let uuid_pop = uuid_pop.clone();
                                move |(cfi, text): (String, String)| {
                                    let mut selection = selection;
                                    selection.set(None);
                                    spawn_create_highlight(
                                        uuid_pop.clone(), cfi, HighlightColor::Amber, text,
                                        highlights, note_target, true,
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
            }

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
                    on_close: move |_| {
                        let mut show_highlights = show_highlights;
                        show_highlights.set(false);
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
}

/// Top navigation bar: back button, title + chapter display, Aa + bookmark tools.
#[component]
fn ReaderTopChrome(
    book_title: String,
    chapter_title: String,
    show_aa: bool,
    toc_active: bool,
    highlights_active: bool,
    highlight_count: usize,
    on_back: EventHandler<MouseEvent>,
    on_toggle_aa: EventHandler<MouseEvent>,
    on_toggle_toc: EventHandler<MouseEvent>,
    on_toggle_highlights: EventHandler<MouseEvent>,
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
                    class: "rd-tool",
                    r#type: "button",
                    "data-testid": "reader-bookmark",
                    "aria-label": "Bookmark (coming soon)",
                    title: "Bookmark — coming soon",
                    disabled: true,
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

#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    // First-paint contract: `BookReadPage` seeds `status` from
    // `ReaderStatus::default()` so SSR and the first WASM render produce
    // identical markup (the `rd-overlay` loading node is present in both).
    // Flipping this default would re-introduce the hydration mismatch
    // described in .claude/rules/07-hydration.md.
    #[test]
    fn reader_status_default_is_loading_for_ssr_wasm_parity() {
        assert_eq!(ReaderStatus::default(), ReaderStatus::Loading);
    }

    #[test]
    fn format_progress_labels_returns_empty_strings_before_first_relocate() {
        let (page, chapter) = format_progress_labels(&RelocateData::default());
        assert_eq!(page, "");
        assert_eq!(chapter, "");
    }

    #[test]
    fn format_progress_labels_formats_page_and_chapter_strings() {
        let data = RelocateData {
            cfi: None,
            page: 42,
            total_pages: 300,
            pct: 14,
            chapter: 3,
            total_chapters: 24,
            chapter_title: String::new(),
        };
        let (page, chapter) = format_progress_labels(&data);
        assert!(page.contains("p."));
        assert!(page.contains("42"));
        assert!(page.contains("300"));
        assert!(page.contains("14%"));
        assert!(chapter.contains("Ch"));
        assert!(chapter.contains("3"));
        assert!(chapter.contains("24"));
    }

    #[test]
    fn format_progress_labels_falls_back_to_pct_only_when_total_pages_unknown() {
        let data = RelocateData {
            cfi: None,
            page: 0,
            total_pages: 0,
            pct: 7,
            chapter: 0,
            total_chapters: 0,
            chapter_title: String::new(),
        };
        let (page, chapter) = format_progress_labels(&data);
        assert_eq!(page, "7%");
        assert_eq!(chapter, "");
    }
}
