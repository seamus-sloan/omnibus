//! F2.2 barebones EPUB reader — immersive, full-screen reading surface.
//!
//! Loads epub.js (+ JSZip) as classic sibling scripts and the vendored
//! `epub-reader-glue.js`, which exposes `window.OmnibusReader`. The Rust side
//! drives it through `dioxus::document::eval`, streams the EPUB bytes from the
//! cookie-gated `GET /api/ebooks/:uuid/file` route, and persists the current
//! position (an opaque EPUB CFI) through [`crate::reader_progress`].
//!
//! The chrome (back, font −/+, theme buttons, prev/next) renders on every
//! target so SSR/mobile builds compile; the JS interop that actually mounts a
//! book is web-only (`#[cfg(feature = "web")]`). Theme changes flow into the
//! reader via a `use_effect` on the shared `Theme` signal so the in-iframe
//! content tracks the app theme.
//!
//! Position persistence is localStorage-only for now (web) — superseded by the
//! server-backed F2.1 progress-sync endpoint when that lands.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;

use crate::components::atrium::{persist_theme, Theme};

// Vendored reader runtime, loaded only on this page (≈300 KB) so the rest of
// the app never pays for it. Order matters: JSZip and epub.js define the
// globals (`JSZip`, `ePub`) that the glue closes over, so they're emitted as
// siblings ahead of the glue script. The readiness-poll in the mount effect
// tolerates their async load order.
const JSZIP_JS: Asset = asset!("/assets/vendor/jszip.min.js");
const EPUBJS_JS: Asset = asset!("/assets/vendor/epub.min.js");
const READER_GLUE_JS: Asset = asset!("/assets/vendor/epub-reader-glue.js");

/// Lifecycle of the in-iframe book render. Drives the loading / error overlay
/// so a slow network, a 404 file route, or a malformed EPUB shows feedback
/// instead of a blank viewer. The glue reports `ready` / `error` through the
/// `__omnibusOnStatus` callback; the readiness poll reports `error` if the
/// vendored scripts never load.
///
/// Only the `web` build transitions through `Loading`/`Failed` (driven by the
/// glue); SSR/mobile have no interop and sit at `Ready`, so those variants read
/// as never-constructed there.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
enum ReaderStatus {
    Loading,
    Ready,
    Failed,
}

/// Single audited surface for calling into `window.OmnibusReader`. `method` is
/// always a hard-coded identifier and `arg_js` is either empty, an integer, or
/// a `serde_json`-encoded literal — so this is the one place JS is built for
/// reader commands (the bootstrap `init` poll below is the only other eval).
#[cfg(feature = "web")]
fn reader_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusReader && window.OmnibusReader.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

#[component]
pub fn BookReadPage(uuid: String) -> Element {
    let theme = use_context::<Signal<Theme>>();

    // Book-render lifecycle. Web starts Loading and the glue/poll resolve it;
    // other targets have no interop, so they render the (empty) viewer as Ready.
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

    // Font size in px; clamped to a sane reading range on adjust. Declared on
    // every target so the chrome buttons can read/write it; only the web build
    // actually pushes it into the reader runtime.
    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut font_size = use_signal(|| 18i32);

    // ── Web interop: mount the reader once the async scripts are ready. ──
    #[cfg(feature = "web")]
    {
        use wasm_bindgen::prelude::*;

        // Component-lifetime owner for the JS callbacks. Replacing the vec on a
        // re-mount (uuid change) drops the previous generation, and the holder
        // drops on unmount — so callbacks are never leaked with `forget()`.
        // `use_hook` hands back a clone of the `Rc` each render; the inner
        // `RefCell<Vec<..>>` is the single live store.
        let cb_holder: std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(String)>>>> =
            use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

        let uuid_for_mount = uuid.clone();
        let uuid_for_cb = uuid.clone();
        use_effect(use_reactive!(|uuid_for_mount| {
            let uuid = uuid_for_mount.clone();
            let uuid_cb = uuid_for_cb.clone();

            // Re-entering a book restarts the lifecycle (Loading until the glue
            // reports ready/error, or the poll times out).
            status.set(ReaderStatus::Loading);

            let saved = crate::reader_progress::load(&uuid);
            let size = font_size();
            let theme_name = theme.read().as_attr();
            // Build every JS literal with serde_json so untrusted input (the
            // route-controlled `uuid`, the persisted `cfi`) can never break out
            // of string context in the `document::eval` payload. `theme` is a
            // fixed enum attr but is encoded the same way for uniformity.
            let url_lit = serde_json::to_string(&format!("/api/ebooks/{uuid}/file"))
                .unwrap_or_else(|_| "\"\"".into());
            let theme_lit = serde_json::to_string(theme_name).unwrap_or_else(|_| "\"dark\"".into());
            let cfi_arg = serde_json::to_string(&saved).unwrap_or_else(|_| "null".into());

            // (Re)build the callbacks the glue invokes — relocate (persist CFI)
            // and status (drive the loading/error overlay) — owning them in the
            // holder so the previous generation is dropped, never leaked.
            if let Some(window) = web_sys::window() {
                let relocate = Closure::<dyn FnMut(String)>::new(move |cfi: String| {
                    crate::reader_progress::save(&uuid_cb, &cfi);
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
                // Window now references the new closures; replacing the holder
                // drops the previous generation's.
                *cb_holder.borrow_mut() = vec![relocate, on_status];
            }

            // Poll until the vendored globals load (async) then init. Bounded
            // (~10s) so a failed asset load reports an error via the status
            // callback instead of spinning on a blank viewer forever.
            let js = format!(
                r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusReader && window.ePub) {{ window.OmnibusReader.init("omnibus-viewer", {url_lit}, {{ cfi: {cfi_arg}, fontSize: {size}, theme: {theme_lit} }}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else if (typeof window.__omnibusOnStatus === "function") {{ window.__omnibusOnStatus("error"); }} }})(); }})();"#
            );
            let _ = dioxus::document::eval(&js);
        }));

        // Flow app theme changes into the reader content.
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

    rsx! {
        // Vendored runtime — emitted ahead of any interop. Loaded only on
        // this page so the rest of the app never ships epub.js.
        document::Script { src: JSZIP_JS }
        document::Script { src: EPUBJS_JS }
        document::Script { src: READER_GLUE_JS }

        div {
            class: "reader-root",
            style: "display:flex; flex-direction:column; height:100vh; width:100%;",

            // Slim top control bar.
            div {
                class: "reader-bar",
                style: "display:flex; align-items:center; gap:0.5rem; padding:0.5rem 0.75rem; border-bottom:1px solid var(--line);",
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    "data-testid": "reader-back",
                    "aria-label": "Back",
                    onclick: on_back,
                    "\u{2190} Back"
                }
                div { style: "flex:1;" }
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    "data-testid": "reader-font-decrease",
                    "aria-label": "Decrease font size",
                    onclick: on_font_decrease,
                    "A\u{2212}"
                }
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    "data-testid": "reader-font-increase",
                    "aria-label": "Increase font size",
                    onclick: on_font_increase,
                    "A+"
                }
                div {
                    class: "reader-theme-seg",
                    style: "display:flex; gap:0.25rem;",
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        "data-testid": "reader-theme-dark",
                        "aria-label": "Dark theme",
                        onclick: move |_| set_theme(Theme::Dark),
                        "Dark"
                    }
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        "data-testid": "reader-theme-light",
                        "aria-label": "Light theme",
                        onclick: move |_| set_theme(Theme::Light),
                        "Light"
                    }
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        "data-testid": "reader-theme-sepia",
                        "aria-label": "Sepia theme",
                        onclick: move |_| set_theme(Theme::Sepia),
                        "Sepia"
                    }
                }
                div { style: "flex:1;" }
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    "data-testid": "reader-prev",
                    "aria-label": "Previous page",
                    onclick: on_prev,
                    "\u{2039} Prev"
                }
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    "data-testid": "reader-next",
                    "aria-label": "Next page",
                    onclick: on_next,
                    "Next \u{203a}"
                }
            }

            // Viewer fills the remaining space; epub.js renders into this id.
            // A status overlay sits on top until the first page paints (or on
            // failure) so a slow/missing/broken book shows feedback, not a
            // blank box.
            div {
                class: "reader-stage",
                style: "flex:1; min-height:0; width:100%; position:relative;",
                div {
                    id: "omnibus-viewer",
                    class: "reader-viewer",
                    "data-testid": "reader-viewer",
                    style: "position:absolute; inset:0;",
                }
                match status() {
                    ReaderStatus::Loading => rsx! {
                        div {
                            class: "reader-overlay",
                            "data-testid": "reader-loading",
                            style: "position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:var(--ink-2); background:var(--bg-0);",
                            "Loading\u{2026}"
                        }
                    },
                    ReaderStatus::Failed => rsx! {
                        div {
                            class: "reader-overlay",
                            "data-testid": "reader-error",
                            role: "alert",
                            style: "position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:var(--ink-2); background:var(--bg-0);",
                            "This book couldn\u{2019}t be loaded."
                        }
                    },
                    ReaderStatus::Ready => rsx! {},
                }
            }
        }
    }
}
