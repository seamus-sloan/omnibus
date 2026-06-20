//! Immersive full-screen EPUB reader. Loads the vendored epub.js +
//! JSZip glue (`window.OmnibusReader`) via `dioxus::document::eval`, streams
//! bytes from cookie-gated `GET /api/ebooks/:uuid/file`, and persists
//! position via [`crate::reader_progress`]. Chrome compiles on every
//! target; the JS interop that mounts a book is web-only.

mod aa_panel;
mod bootstrap;
mod selection;
mod typography;

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;

use crate::components::atrium::{persist_theme, Theme};
use crate::contexts::use_server_url;
use crate::data;

use omnibus_shared::{EbookMetadata, Highlight, HighlightColor};

use aa_panel::ReaderAaPanel;
#[cfg(feature = "web")]
use bootstrap::{reader_bootstrap_js, BootstrapArgs};
use selection::{SelectionData, SelectionPopover};
#[cfg(feature = "web")]
use typography::{load_reader_pref, save_reader_pref};
use typography::{LineSpacing, Margins, Typeface};

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
struct RelocateData {
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

/// Extract `file_id` from the current URL's query string (`?file_id=N`),
/// targeting a specific `book_files` row for multi-file books.
#[cfg(feature = "web")]
fn parse_file_id_from_url() -> Option<i64> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("file_id")?.parse().ok()
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
    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut status = use_signal(ReaderStatus::default);

    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut font_size = use_signal(|| 18i32);

    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut typeface = use_signal(|| {
        #[cfg(feature = "web")]
        {
            load_reader_pref("omn.typeface")
                .and_then(|s| Typeface::from_storage(&s))
                .unwrap_or(Typeface::Editorial)
        }
        #[cfg(not(feature = "web"))]
        Typeface::Editorial
    });
    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut line_spacing = use_signal(|| {
        #[cfg(feature = "web")]
        {
            load_reader_pref("omn.lineSpacing")
                .and_then(|s| LineSpacing::from_storage(&s))
                .unwrap_or(LineSpacing::Cozy)
        }
        #[cfg(not(feature = "web"))]
        LineSpacing::Cozy
    });
    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut margins = use_signal(|| {
        #[cfg(feature = "web")]
        {
            load_reader_pref("omn.margins")
                .and_then(|s| Margins::from_storage(&s))
                .unwrap_or(Margins::Normal)
        }
        #[cfg(not(feature = "web"))]
        Margins::Normal
    });
    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut justify = use_signal(|| {
        #[cfg(feature = "web")]
        {
            load_reader_pref("omn.justify").as_deref() == Some("true")
        }
        #[cfg(not(feature = "web"))]
        false
    });

    let mut show_aa = use_signal(|| false);

    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut selection: Signal<Option<SelectionData>> = use_signal(|| None);

    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut highlights: Signal<Vec<Highlight>> = use_signal(Vec::new);

    // Relocated data from epub.js (page, chapter, pct).
    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut loc = use_signal(RelocateData::default);

    // Book metadata for the top-bar title.
    let book_meta: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let server_url = use_server_url();
    let uuid_for_meta = uuid.clone();
    {
        let mut book_meta = book_meta;
        use_effect(use_reactive!(|uuid_for_meta| {
            // Clear stale title immediately so SPA navigations between books
            // don't flash the previous book's name while the fetch is in flight.
            book_meta.set(None);
            let url = server_url.clone();
            let uuid = uuid_for_meta.clone();
            spawn(async move {
                if let Ok(Some(b)) = data::get_ebook(&url, &uuid).await {
                    book_meta.set(Some(b));
                }
            });
        }));
    }

    // ── Web interop: mount the reader once the async scripts are ready. ──
    #[cfg(feature = "web")]
    {
        use wasm_bindgen::prelude::*;

        let cb_holder: std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(String)>>>> =
            use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

        let uuid_for_mount = uuid.clone();
        let uuid_for_cb = uuid.clone();
        use_effect(use_reactive!(|uuid_for_mount| {
            let uuid = uuid_for_mount.clone();
            let uuid_cb = uuid_for_cb.clone();
            status.set(ReaderStatus::Loading);

            let local_saved = crate::reader_progress::load(&uuid);
            let size = font_size();
            let theme_name = theme.read().as_attr();
            let file_url = match parse_file_id_from_url() {
                Some(fid) => format!("/api/ebooks/{uuid}/file?file_id={fid}"),
                None => format!("/api/ebooks/{uuid}/file"),
            };
            let url_lit = serde_json::to_string(&file_url).unwrap_or_else(|_| "\"\"".into());
            let theme_lit = serde_json::to_string(theme_name).unwrap_or_else(|_| "\"dark\"".into());
            let font_family_lit =
                serde_json::to_string(typeface().to_css()).unwrap_or_else(|_| "null".into());
            let line_height_lit =
                serde_json::to_string(line_spacing().to_css()).unwrap_or_else(|_| "null".into());
            let max_width_lit =
                serde_json::to_string(margins().to_css()).unwrap_or_else(|_| "null".into());
            let justify_val = justify();

            if let Some(window) = web_sys::window() {
                let uuid_for_save = uuid_cb.clone();
                let relocate = Closure::<dyn FnMut(String)>::new(move |json: String| {
                    if let Ok(data) = serde_json::from_str::<RelocateData>(&json) {
                        if let Some(ref cfi) = data.cfi {
                            crate::reader_progress::save(&uuid_for_save, cfi);
                            let uuid_for_post = uuid_for_save.clone();
                            let cfi_for_post = cfi.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                let body = serde_json::json!({
                                    "update": {
                                        "book_uuid": uuid_for_post,
                                        "format": "epub",
                                        "epub_cfi": cfi_for_post,
                                    }
                                });
                                if let Ok(req) =
                                    gloo_net::http::Request::post("/api/rpc/progress").json(&body)
                                {
                                    let _ = req.send().await;
                                }
                            });
                        }
                        loc.set(data);
                    }
                });
                let on_status = Closure::<dyn FnMut(String)>::new(move |state: String| {
                    status.set(match state.as_str() {
                        "ready" => ReaderStatus::Ready,
                        "error" => ReaderStatus::Failed,
                        _ => ReaderStatus::Loading,
                    });
                });
                let _ = js_sys::Reflect::set(
                    &window,
                    &JsValue::from_str("__omnibusOnRelocate"),
                    relocate.as_ref().unchecked_ref(),
                );
                let on_selection = Closure::<dyn FnMut(String)>::new(move |json: String| {
                    if let Ok(data) = serde_json::from_str::<SelectionData>(&json) {
                        selection.set(Some(data));
                    }
                });
                let _ = js_sys::Reflect::set(
                    &window,
                    &JsValue::from_str("__omnibusOnStatus"),
                    on_status.as_ref().unchecked_ref(),
                );
                let _ = js_sys::Reflect::set(
                    &window,
                    &JsValue::from_str("__omnibusOnSelection"),
                    on_selection.as_ref().unchecked_ref(),
                );
                *cb_holder.borrow_mut() = vec![relocate, on_status, on_selection];
            }

            let uuid_for_fetch = uuid.clone();
            spawn(async move {
                let server_cfi = crate::data::get_progress(
                    "",
                    &uuid_for_fetch,
                    omnibus_shared::ProgressFormat::Epub,
                )
                .await
                .ok()
                .flatten()
                .and_then(|r| r.epub_cfi);
                let chosen = server_cfi.or(local_saved);
                let cfi_arg = serde_json::to_string(&chosen).unwrap_or_else(|_| "null".into());
                let js = reader_bootstrap_js(&BootstrapArgs {
                    url_lit: &url_lit,
                    cfi_arg: &cfi_arg,
                    font_size: size,
                    theme_lit: &theme_lit,
                    font_family_lit: &font_family_lit,
                    line_height_lit: &line_height_lit,
                    max_width_lit: &max_width_lit,
                    justify_val,
                });
                let _ = dioxus::document::eval(&js);

                let hl_uuid = uuid_for_fetch.clone();
                if let Ok(list) = data::list_highlights("", &hl_uuid).await {
                    for h in &list {
                        let cfi_lit = serde_json::to_string(&h.epub_cfi_range)
                            .unwrap_or_else(|_| "\"\"".into());
                        let color_lit = serde_json::to_string(h.color.as_str())
                            .unwrap_or_else(|_| "\"amber\"".into());
                        reader_call("addAnnotation", &format!("{cfi_lit}, {color_lit}"));
                    }
                    highlights.set(list);
                }
            });
        }));

        use_effect(move || {
            let attr_lit =
                serde_json::to_string(theme.read().as_attr()).unwrap_or_else(|_| "\"dark\"".into());
            reader_call("setTheme", &attr_lit);
        });
    }

    // ── Chrome handlers ─────────────────────────────────────────────────
    #[cfg(not(feature = "mobile"))]
    let nav = use_navigator();

    let on_back = move |_| {
        #[cfg(not(feature = "mobile"))]
        nav.go_back();
    };

    let on_font_decrease = move |_| {
        let next = (font_size() - 1).clamp(12, 32);
        font_size.set(next);
        #[cfg(feature = "web")]
        reader_call("setFontSize", &next.to_string());
    };
    let on_font_increase = move |_| {
        let next = (font_size() + 1).clamp(12, 32);
        font_size.set(next);
        #[cfg(feature = "web")]
        reader_call("setFontSize", &next.to_string());
    };

    let on_set_typeface = move |t: Typeface| {
        typeface.set(t);
        #[cfg(feature = "web")]
        {
            let lit = serde_json::to_string(t.to_css()).unwrap_or_else(|_| "null".into());
            reader_call("setFont", &lit);
            save_reader_pref("omn.typeface", t.to_storage());
        }
    };
    let on_set_line_spacing = move |ls: LineSpacing| {
        line_spacing.set(ls);
        #[cfg(feature = "web")]
        {
            let lit = serde_json::to_string(ls.to_css()).unwrap_or_else(|_| "null".into());
            reader_call("setLineHeight", &lit);
            save_reader_pref("omn.lineSpacing", ls.to_storage());
        }
    };
    let on_set_margins = move |m: Margins| {
        margins.set(m);
        #[cfg(feature = "web")]
        {
            let lit = serde_json::to_string(m.to_css()).unwrap_or_else(|_| "null".into());
            reader_call("setMargins", &lit);
            save_reader_pref("omn.margins", m.to_storage());
        }
    };
    let on_toggle_justify = move |_: MouseEvent| {
        let next = !justify();
        justify.set(next);
        #[cfg(feature = "web")]
        {
            reader_call("setJustify", if next { "true" } else { "false" });
            save_reader_pref("omn.justify", if next { "true" } else { "false" });
        }
    };

    let on_prev = move |_| {
        #[cfg(feature = "web")]
        {
            selection.set(None);
            reader_call("prev", "");
        }
    };
    let on_next = move |_| {
        #[cfg(feature = "web")]
        {
            selection.set(None);
            reader_call("next", "");
        }
    };

    let set_theme = move |t: Theme| {
        let mut theme = theme;
        theme.set(t);
        persist_theme(t);
    };

    let on_keydown = move |evt: KeyboardEvent| match evt.key() {
        Key::ArrowLeft => {
            evt.prevent_default();
            #[cfg(feature = "web")]
            reader_call("prev", "");
        }
        Key::ArrowRight => {
            evt.prevent_default();
            #[cfg(feature = "web")]
            reader_call("next", "");
        }
        Key::Escape => {
            evt.prevent_default();
            if selection.read().is_some() {
                selection.set(None);
            } else if show_aa() {
                show_aa.set(false);
            } else {
                #[cfg(not(feature = "mobile"))]
                nav.go_back();
            }
        }
        _ => {}
    };

    let current = loc.read();
    let pct = current.pct;
    let page_str = if current.total_pages > 0 {
        format!(
            "p.\u{a0}{} of {}\u{a0}\u{b7}\u{a0}{}%",
            current.page, current.total_pages, pct
        )
    } else if pct > 0 {
        format!("{}%", pct)
    } else {
        String::new()
    };
    let chapter_str = if current.total_chapters > 0 {
        format!("Ch\u{a0}{} of {}", current.chapter, current.total_chapters)
    } else {
        String::new()
    };
    let chapter_title_display = current.chapter_title.clone();

    let book_title = book_meta
        .read()
        .as_ref()
        .and_then(|b| b.title.clone())
        .unwrap_or_default();

    // Font size is a small i32 slider value in `[12, 32]`; the `as f32`
    // cast is exact within that range.
    #[allow(clippy::cast_precision_loss)]
    let font_offset = (font_size() - 12) as f32;
    let font_pct = (font_offset / 20.0 * 100.0).clamp(0.0, 100.0);

    rsx! {
        ReaderLayout {
            uuid: uuid.clone(),
            book_title,
            chapter_title: chapter_title_display,
            page_str,
            chapter_str,
            pct,
            status: status(),
            theme: *theme.read(),
            font_pct,
            typeface: typeface(),
            line_spacing: line_spacing(),
            margins: margins(),
            justify: justify(),
            show_aa,
            selection,
            highlights,
            on_keydown,
            on_back,
            on_prev,
            on_next,
            on_set_theme: set_theme,
            on_font_decrease,
            on_font_increase,
            on_set_typeface,
            on_set_line_spacing,
            on_set_margins,
            on_toggle_justify,
        }
    }
}

/// Reader chrome + panels (top bar, viewer stage, gutters, status bar, Aa panel, selection popover).
#[component]
#[allow(clippy::too_many_arguments)]
fn ReaderLayout(
    uuid: String,
    book_title: String,
    chapter_title: String,
    page_str: String,
    chapter_str: String,
    pct: u32,
    status: ReaderStatus,
    theme: Theme,
    font_pct: f32,
    typeface: Typeface,
    line_spacing: LineSpacing,
    margins: Margins,
    justify: bool,
    show_aa: Signal<bool>,
    selection: Signal<Option<SelectionData>>,
    highlights: Signal<Vec<Highlight>>,
    on_keydown: EventHandler<KeyboardEvent>,
    on_back: EventHandler<MouseEvent>,
    on_prev: EventHandler<MouseEvent>,
    on_next: EventHandler<MouseEvent>,
    on_set_theme: EventHandler<Theme>,
    on_font_decrease: EventHandler<MouseEvent>,
    on_font_increase: EventHandler<MouseEvent>,
    on_set_typeface: EventHandler<Typeface>,
    on_set_line_spacing: EventHandler<LineSpacing>,
    on_set_margins: EventHandler<Margins>,
    on_toggle_justify: EventHandler<MouseEvent>,
) -> Element {
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
                on_back,
                on_toggle_aa: move |_| {
                    let mut show_aa = show_aa;
                    show_aa.set(!show_aa());
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
                    theme,
                    font_pct,
                    typeface,
                    line_spacing,
                    margins,
                    justify,
                    on_set_theme,
                    on_font_decrease,
                    on_font_increase,
                    on_set_typeface,
                    on_set_line_spacing,
                    on_set_margins,
                    on_toggle_justify,
                    on_close: move |_| {
                        let mut show_aa = show_aa;
                        show_aa.set(false);
                    },
                }
            }

            if let Some(sel) = selection.read().as_ref() {
                SelectionPopover {
                    sel_rect_x: sel.rect.x,
                    sel_rect_y: sel.rect.y,
                    sel_rect_width: sel.rect.width,
                    sel_cfi: sel.cfi_range.clone(),
                    on_dismiss: move |_| {
                        let mut selection = selection;
                        selection.set(None);
                    },
                    on_highlight: move |(cfi, color): (String, HighlightColor)| {
                        #[cfg(feature = "web")]
                        {
                            let cfi_lit = serde_json::to_string(&cfi)
                                .unwrap_or_else(|_| "\"\"".into());
                            let color_lit = serde_json::to_string(color.as_str())
                                .unwrap_or_else(|_| "\"amber\"".into());
                            reader_call("addAnnotation", &format!("{cfi_lit}, {color_lit}"));
                        }

                        let create = omnibus_shared::CreateHighlight {
                            book_uuid: uuid.clone(),
                            epub_cfi_range: cfi.clone(),
                            color,
                        };
                        #[cfg(feature = "web")]
                        let cfi_rollback = cfi.clone();
                        let mut selection = selection;
                        let mut highlights = highlights;
                        selection.set(None);
                        spawn(async move {
                            if let Ok(h) = data::create_highlight("", create).await {
                                highlights.write().push(h);
                            } else {
                                // The annotation was added optimistically above;
                                // if the write didn't land, roll it back so a
                                // failed create doesn't leave a highlight that
                                // vanishes on the next reload.
                                #[cfg(feature = "web")]
                                {
                                    let cfi_lit = serde_json::to_string(&cfi_rollback)
                                        .unwrap_or_else(|_| "\"\"".into());
                                    reader_call("removeAnnotation", &cfi_lit);
                                }
                            }
                        });
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
    on_back: EventHandler<MouseEvent>,
    on_toggle_aa: EventHandler<MouseEvent>,
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
                    class: if show_aa { "rd-tool rd-aa on" } else { "rd-tool rd-aa" },
                    r#type: "button",
                    "data-testid": "reader-aa",
                    "aria-label": "Display settings",
                    onclick: on_toggle_aa,
                    "Aa"
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
}
