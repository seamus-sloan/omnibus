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

#[component]
pub fn BookReadPage(uuid: String) -> Element {
    let theme = use_context::<Signal<Theme>>();

    // Font size in px; clamped to a sane reading range on adjust. Declared on
    // every target so the chrome buttons can read/write it; only the web build
    // actually pushes it into the reader runtime.
    #[cfg_attr(not(feature = "web"), allow(unused_variables, unused_mut))]
    let mut font_size = use_signal(|| 18i32);

    // ── Web interop: mount the reader once the async scripts are ready. ──
    #[cfg(feature = "web")]
    {
        let uuid_for_mount = uuid.clone();
        let uuid_for_cb = uuid.clone();
        // Run once after first render. `use_effect` with no signal reads only
        // re-runs when its tracked deps change — there are none, so this is a
        // mount-time effect.
        use_effect(use_reactive!(|uuid_for_mount| {
            use wasm_bindgen::prelude::*;

            let uuid = uuid_for_mount.clone();
            let uuid_cb = uuid_for_cb.clone();
            let saved = crate::reader_progress::load(&uuid);
            let size = font_size();
            let theme_name = theme.read().as_attr();

            // Register the relocate callback the glue invokes on every page
            // turn: persist the new CFI so a re-open resumes in place. Leaked
            // for the page lifetime (`forget`) — there's a single reader per
            // page mount and the window outlives it.
            if let Some(window) = web_sys::window() {
                let closure = Closure::<dyn FnMut(String)>::new(move |cfi: String| {
                    crate::reader_progress::save(&uuid_cb, &cfi);
                });
                let _ = js_sys::Reflect::set(
                    &window,
                    &JsValue::from_str("__omnibusOnRelocate"),
                    closure.as_ref().unchecked_ref(),
                );
                closure.forget();
            }

            // Build a JSON-safe CFI literal (`"…"` or `null`) for the init
            // call. Escape backslashes first, then double-quotes, so the
            // generated JS string literal is valid for arbitrary CFI text.
            let cfi_arg = saved
                .map(|c| format!("\"{}\"", c.replace('\\', "\\\\").replace('"', "\\\"")))
                .unwrap_or_else(|| "null".into());

            // Poll until both globals exist (scripts load async) then init,
            // pointing the reader at the cookie-gated file route.
            let js = format!(
                r#"(function go(){{ if (window.OmnibusReader && window.ePub) {{ window.OmnibusReader.init("omnibus-viewer", "/api/ebooks/{uuid}/file", {{ cfi: {cfi_arg}, fontSize: {size}, theme: "{theme_name}" }}); }} else {{ setTimeout(go, 50); }} }})();"#
            );
            let _ = dioxus::document::eval(&js);
        }));

        // Flow app theme changes into the reader content.
        use_effect(move || {
            let attr = theme.read().as_attr();
            let js = format!(r#"window.OmnibusReader && window.OmnibusReader.setTheme("{attr}");"#);
            let _ = dioxus::document::eval(&js);
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
        {
            let js = format!("window.OmnibusReader && window.OmnibusReader.setFontSize({next});");
            let _ = dioxus::document::eval(&js);
        }
    };
    let on_font_increase = move |_| {
        let next = (font_size() + 1).clamp(12, 32);
        font_size.set(next);
        #[cfg(feature = "web")]
        {
            let js = format!("window.OmnibusReader && window.OmnibusReader.setFontSize({next});");
            let _ = dioxus::document::eval(&js);
        }
    };

    let on_prev = move |_| {
        #[cfg(feature = "web")]
        {
            let _ = dioxus::document::eval("window.OmnibusReader && window.OmnibusReader.prev();");
        }
    };
    let on_next = move |_| {
        #[cfg(feature = "web")]
        {
            let _ = dioxus::document::eval("window.OmnibusReader && window.OmnibusReader.next();");
        }
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
            div {
                id: "omnibus-viewer",
                class: "reader-viewer",
                "data-testid": "reader-viewer",
                style: "flex:1; min-height:0; width:100%;",
            }
        }
    }
}
