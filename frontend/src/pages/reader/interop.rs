//! Web-only JS interop for the reader: registers `__omnibusOn*` callbacks
//! on `window`, primes the epub.js viewer via the bootstrap IIFE, and
//! seeds the annotation layer from the saved highlights. Extracted from
//! `BookReadPage` so the parent reads as plain Rust signal/state wiring.

use dioxus::prelude::*;

use omnibus_shared::Highlight;

use super::prefs::ReaderPrefs;
use super::selection::SelectionData;
use super::ReaderStatus;

/// All the signals the reader needs to drive from JS callbacks; passed as
/// a struct so the call site reads as one named argument rather than
/// seven positional ones.
#[cfg(feature = "web")]
#[derive(Copy, Clone)]
pub(crate) struct InteropSignals {
    pub status: Signal<ReaderStatus>,
    pub loc: Signal<super::RelocateData>,
    pub selection: Signal<Option<SelectionData>>,
    pub highlights: Signal<Vec<Highlight>>,
}

/// Boxed list of the `__omnibusOn*` window callbacks. Held on the heap
/// so the `Closure`s outlive the `use_effect` that registered them.
#[cfg(feature = "web")]
type CallbackHolder =
    std::rc::Rc<std::cell::RefCell<Vec<wasm_bindgen::prelude::Closure<dyn FnMut(String)>>>>;

/// Install the web JS interop: register window callbacks, mount epub.js
/// via the bootstrap IIFE on every `uuid` change, and stream the saved
/// highlights into the viewer. No-op on non-web targets.
#[cfg(feature = "web")]
pub(crate) fn install_reader_web_interop(uuid: String, prefs: ReaderPrefs, sigs: InteropSignals) {
    use wasm_bindgen::prelude::*;

    use super::bootstrap::{reader_bootstrap_js, BootstrapArgs};
    use super::parse_file_id_from_url;
    use super::reader_call;
    use crate::data;

    let cb_holder: CallbackHolder =
        use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

    let theme = prefs.theme;
    let InteropSignals {
        mut status,
        mut loc,
        mut selection,
        mut highlights,
    } = sigs;

    let uuid_for_mount = uuid.clone();
    let uuid_for_cb = uuid;
    use_effect(use_reactive!(|uuid_for_mount| {
        let uuid = uuid_for_mount.clone();
        let uuid_cb = uuid_for_cb.clone();
        status.set(ReaderStatus::Loading);

        let local_saved = crate::reader_progress::load(&uuid);
        let size = *prefs.font_size.read();
        let theme_name = theme.read().as_attr();
        let file_url = match parse_file_id_from_url() {
            Some(fid) => format!("/api/ebooks/{uuid}/file?file_id={fid}"),
            None => format!("/api/ebooks/{uuid}/file"),
        };
        let url_lit = serde_json::to_string(&file_url).unwrap_or_else(|_| "\"\"".into());
        let theme_lit = serde_json::to_string(theme_name).unwrap_or_else(|_| "\"dark\"".into());
        let font_family_lit =
            serde_json::to_string(prefs.typeface.read().to_css()).unwrap_or_else(|_| "null".into());
        let line_height_lit = serde_json::to_string(prefs.line_spacing.read().to_css())
            .unwrap_or_else(|_| "null".into());
        let max_width_lit =
            serde_json::to_string(prefs.margins.read().to_css()).unwrap_or_else(|_| "null".into());
        let justify_val = *prefs.justify.read();

        if let Some(window) = web_sys::window() {
            let uuid_for_save = uuid_cb.clone();
            let relocate = Closure::<dyn FnMut(String)>::new(move |json: String| {
                if let Ok(data) = serde_json::from_str::<super::RelocateData>(&json) {
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
                    let cfi_lit =
                        serde_json::to_string(&h.epub_cfi_range).unwrap_or_else(|_| "\"\"".into());
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
