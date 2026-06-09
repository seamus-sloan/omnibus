//! Immersive full-screen EPUB reader (F2.4). Loads the vendored epub.js +
//! JSZip glue (`window.OmnibusReader`) via `dioxus::document::eval`, streams
//! bytes from cookie-gated `GET /api/ebooks/:uuid/file`, and persists
//! position via [`crate::reader_progress`]. Chrome compiles on every
//! target; the JS interop that mounts a book is web-only.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;

use crate::components::atrium::{persist_theme, Theme};
use crate::contexts::use_server_url;
use crate::data;

use omnibus_shared::EbookMetadata;

const JSZIP_JS: Asset = asset!("/assets/vendor/jszip.min.js");
const EPUBJS_JS: Asset = asset!("/assets/vendor/epub.min.js");
const READER_GLUE_JS: Asset = asset!("/assets/vendor/epub-reader-glue.js");

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
enum ReaderStatus {
    Loading,
    Ready,
    Failed,
}

/// Relocated event data from epub.js glue (deserialized from JSON).
#[derive(Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct RelocateData {
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

/// Full-screen EPUB reader page (web-feature interop, all-target chrome).
#[component]
pub fn BookReadPage(uuid: String) -> Element {
    let theme = use_context::<Signal<Theme>>();

    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut status = use_signal(|| {
        #[cfg(feature = "web")]
        {
            ReaderStatus::Loading
        }
        #[cfg(not(feature = "web"))]
        {
            ReaderStatus::Ready
        }
    });

    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut font_size = use_signal(|| 18i32);

    let mut show_aa = use_signal(|| false);

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
            let url_lit = serde_json::to_string(&format!("/api/ebooks/{uuid}/file"))
                .unwrap_or_else(|_| "\"\"".into());
            let theme_lit = serde_json::to_string(theme_name).unwrap_or_else(|_| "\"dark\"".into());

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
                let _ = js_sys::Reflect::set(
                    &window,
                    &JsValue::from_str("__omnibusOnStatus"),
                    on_status.as_ref().unchecked_ref(),
                );
                *cb_holder.borrow_mut() = vec![relocate, on_status];
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
                let js = format!(
                    r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusReader && window.ePub) {{ window.OmnibusReader.init("omnibus-viewer", {url_lit}, {{ cfi: {cfi_arg}, fontSize: {size}, theme: {theme_lit} }}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else if (typeof window.__omnibusOnStatus === "function") {{ window.__omnibusOnStatus("error"); }} }})(); }})();"#
                );
                let _ = dioxus::document::eval(&js);
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

    let on_prev = move |_| {
        #[cfg(feature = "web")]
        reader_call("prev", "");
    };
    let on_next = move |_| {
        #[cfg(feature = "web")]
        reader_call("next", "");
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
            if show_aa() {
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

    let font_pct = ((font_size() - 12) as f32 / 20.0 * 100.0).clamp(0.0, 100.0);

    rsx! {
        document::Script { src: JSZIP_JS }
        document::Script { src: EPUBJS_JS }
        document::Script { src: READER_GLUE_JS }

        div {
            class: "rd-surface",
            tabindex: "0",
            autofocus: true,
            onkeydown: on_keydown,

            div { class: "rd-wash" }

            ReaderTopChrome {
                book_title: book_title.clone(),
                chapter_title: chapter_title_display.clone(),
                show_aa: show_aa(),
                on_back,
                on_toggle_aa: move |_| show_aa.set(!show_aa()),
            }

            ReaderViewerStage { status: status() }

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
                    theme: *theme.read(),
                    font_pct,
                    on_set_theme: set_theme,
                    on_font_decrease,
                    on_font_increase,
                    on_close: move |_| show_aa.set(false),
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

/// Frosted-glass typography settings panel: theme switcher, typeface,
/// text size, line spacing, margins, and justify toggle.
#[component]
fn ReaderAaPanel(
    theme: Theme,
    font_pct: f32,
    on_set_theme: EventHandler<Theme>,
    on_font_decrease: EventHandler<MouseEvent>,
    on_font_increase: EventHandler<MouseEvent>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "rd-scrim", onclick: on_close }
        div {
            class: "rd-aa-panel",
            onclick: move |evt: MouseEvent| evt.stop_propagation(),

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Theme" }
                div {
                    class: "rd-seg",
                    button {
                        class: if theme == Theme::Dark { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_theme.call(Theme::Dark),
                        "Dark"
                    }
                    button {
                        class: if theme == Theme::Light { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_theme.call(Theme::Light),
                        "Light"
                    }
                    button {
                        class: if theme == Theme::Sepia { "on" } else { "" },
                        r#type: "button",
                        onclick: move |_| on_set_theme.call(Theme::Sepia),
                        "Sepia"
                    }
                }
            }

            // Typeface (visual only).
            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Typeface" }
                div {
                    style: "display:flex; gap:6px;",
                    button {
                        class: "rd-typeface-chip on",
                        r#type: "button",
                        span { class: "preview", style: "font-family:'Instrument Serif',serif;", "Aa" }
                        span { class: "name", "Editorial" }
                    }
                    button {
                        class: "rd-typeface-chip",
                        r#type: "button",
                        span { class: "preview", style: "font-family:'EB Garamond',serif;", "Aa" }
                        span { class: "name", "Classic" }
                    }
                    button {
                        class: "rd-typeface-chip",
                        r#type: "button",
                        span { class: "preview", style: "font-family:Georgia,serif;", "Aa" }
                        span { class: "name", "Modern" }
                    }
                }
            }

            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Text size" }
                div {
                    style: "display:flex; align-items:center; gap:12px;",
                    button {
                        class: "rd-tool",
                        r#type: "button",
                        "aria-label": "Decrease font size",
                        "data-testid": "reader-font-decrease",
                        onclick: on_font_decrease,
                        style: "font-family:var(--serif); font-size:13px; color:var(--ink-2); min-width:24px; height:24px; padding:0;",
                        "A"
                    }
                    div {
                        class: "rd-size-track",
                        div { class: "rd-size-fill", style: "width:{font_pct}%;" }
                        div { class: "rd-size-thumb", style: "left:{font_pct}%;" }
                    }
                    button {
                        class: "rd-tool",
                        r#type: "button",
                        "aria-label": "Increase font size",
                        "data-testid": "reader-font-increase",
                        onclick: on_font_increase,
                        style: "font-family:var(--serif); font-size:24px; color:var(--ink-1); min-width:24px; height:24px; padding:0;",
                        "A"
                    }
                }
            }

            // Line spacing (visual only).
            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Line spacing" }
                div {
                    class: "rd-seg",
                    button { r#type: "button", "Tight" }
                    button { class: "on", r#type: "button", "Cozy" }
                    button { r#type: "button", "Airy" }
                }
            }

            // Margins (visual only).
            div { class: "rd-aa-row",
                div { class: "rd-aa-label", "Margins" }
                div {
                    class: "rd-seg",
                    button { r#type: "button", "Narrow" }
                    button { class: "on", r#type: "button", "Normal" }
                    button { r#type: "button", "Wide" }
                }
            }

            // Justify toggle (visual only).
            div {
                class: "rd-toggle-row",
                span { style: "font-size:13px; color:var(--ink-1);", "Justify text" }
                div {
                    class: "rd-toggle-track",
                    div { class: "rd-toggle-knob" }
                }
            }
        }
    }
}
