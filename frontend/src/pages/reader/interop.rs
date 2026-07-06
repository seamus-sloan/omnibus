//! Web-only JS interop for the reader: registers `__omnibusOn*` callbacks
//! on `window`, primes the epub.js viewer via the bootstrap IIFE, and
//! seeds the annotation layer from the saved highlights. Extracted from
//! `BookReadPage` so the parent reads as plain Rust signal/state wiring.

use dioxus::prelude::*;

use omnibus_shared::Highlight;

use super::prefs::ReaderPrefs;
use super::search_panel::SearchResult;
use super::selection::SelectionData;
use super::toc_drawer::TocEntry;
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
    pub toc: Signal<Vec<TocEntry>>,
    pub search_results: Signal<Vec<SearchResult>>,
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
    use super::reader_call;

    let cb_holder: CallbackHolder =
        use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

    let theme = prefs.theme;
    // `sigs` is `Copy`; the callbacks it feeds are wired up in
    // `register_window_callbacks`. Only `status` (initial Loading) and
    // `highlights` (seeded after bootstrap) are driven from this body.
    let mut status = sigs.status;
    let highlights = sigs.highlights;

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
        let spread_lit = serde_json::to_string(prefs.spread.read().to_css())
            .unwrap_or_else(|_| "\"auto\"".into());

        if let Some(window) = web_sys::window() {
            *cb_holder.borrow_mut() = register_window_callbacks(&window, uuid_cb.clone(), sigs);
        }

        let bootstrap = BootstrapLiterals {
            url_lit,
            theme_lit,
            font_family_lit,
            line_height_lit,
            max_width_lit,
            spread_lit,
            font_size: size,
            justify_val,
        };
        spawn(spawn_bootstrap_and_highlights(
            uuid.clone(),
            local_saved,
            bootstrap,
            highlights,
        ));
    }));

    use_effect(move || {
        let attr_lit =
            serde_json::to_string(theme.read().as_attr()).unwrap_or_else(|_| "\"dark\"".into());
        reader_call("setTheme", &attr_lit);
    });
}

/// Register the `__omnibusOn*` window callbacks that stream epub.js events
/// (relocate/status/selection/toc/search) back into the reader signals, and
/// return the owning `Closure`s so the caller can keep them alive past this
/// call. The relocate callback also persists progress locally and POSTs it.
#[cfg(feature = "web")]
fn register_window_callbacks(
    window: &web_sys::Window,
    uuid_cb: String,
    sigs: InteropSignals,
) -> Vec<wasm_bindgen::prelude::Closure<dyn FnMut(String)>> {
    use wasm_bindgen::prelude::*;

    let InteropSignals {
        mut status,
        mut loc,
        mut selection,
        mut toc,
        mut search_results,
        ..
    } = sigs;

    let uuid_for_save = uuid_cb;
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
                    if let Ok(req) = gloo_net::http::Request::post("/api/rpc/progress").json(&body)
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
    let on_selection = Closure::<dyn FnMut(String)>::new(move |json: String| {
        if let Ok(data) = serde_json::from_str::<SelectionData>(&json) {
            selection.set(Some(data));
        }
    });
    let on_toc = Closure::<dyn FnMut(String)>::new(move |json: String| {
        if let Ok(entries) = serde_json::from_str::<Vec<TocEntry>>(&json) {
            toc.set(entries);
        }
    });
    let on_search = Closure::<dyn FnMut(String)>::new(move |json: String| {
        if let Ok(rs) = serde_json::from_str::<Vec<SearchResult>>(&json) {
            search_results.set(rs);
        }
    });
    for (name, cb) in [
        ("__omnibusOnRelocate", &relocate),
        ("__omnibusOnStatus", &on_status),
        ("__omnibusOnSelection", &on_selection),
        ("__omnibusOnToc", &on_toc),
        ("__omnibusOnSearchResults", &on_search),
    ] {
        let _ = js_sys::Reflect::set(
            window,
            &JsValue::from_str(name),
            cb.as_ref().unchecked_ref(),
        );
    }
    vec![relocate, on_status, on_selection, on_toc, on_search]
}

/// The pre-serialized JS literals + scalars the bootstrap IIFE needs, bundled
/// so [`spawn_bootstrap_and_highlights`] takes one named argument.
#[cfg(feature = "web")]
struct BootstrapLiterals {
    url_lit: String,
    theme_lit: String,
    font_family_lit: String,
    line_height_lit: String,
    max_width_lit: String,
    spread_lit: String,
    font_size: i32,
    justify_val: bool,
}

/// Resolve the starting CFI (server progress, else local save), run the
/// epub.js bootstrap IIFE, then seed the annotation layer from the saved
/// highlights and publish them into the `highlights` signal.
#[cfg(feature = "web")]
async fn spawn_bootstrap_and_highlights(
    uuid: String,
    local_saved: Option<String>,
    lits: BootstrapLiterals,
    mut highlights: Signal<Vec<Highlight>>,
) {
    use super::bootstrap::{reader_bootstrap_js, BootstrapArgs};
    use super::reader_call;
    use crate::data;

    let server_cfi = crate::data::get_progress("", &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.epub_cfi);
    let chosen = server_cfi.or(local_saved);
    let cfi_arg = serde_json::to_string(&chosen).unwrap_or_else(|_| "null".into());
    let js = reader_bootstrap_js(&BootstrapArgs {
        url_lit: &lits.url_lit,
        cfi_arg: &cfi_arg,
        font_size: lits.font_size,
        theme_lit: &lits.theme_lit,
        font_family_lit: &lits.font_family_lit,
        line_height_lit: &lits.line_height_lit,
        max_width_lit: &lits.max_width_lit,
        justify_val: lits.justify_val,
        spread_lit: &lits.spread_lit,
    });
    let _ = dioxus::document::eval(&js);

    if let Ok(list) = data::list_highlights("", &uuid).await {
        for h in &list {
            let cfi_lit =
                serde_json::to_string(&h.epub_cfi_range).unwrap_or_else(|_| "\"\"".into());
            let color_lit =
                serde_json::to_string(h.color.as_str()).unwrap_or_else(|_| "\"amber\"".into());
            reader_call("addAnnotation", &format!("{cfi_lit}, {color_lit}"));
        }
        highlights.set(list);
    }
}

/// Extract `file_id` from the current URL's query string (`?file_id=N`),
/// targeting a specific `book_files` row for multi-file books.
#[cfg(feature = "web")]
fn parse_file_id_from_url() -> Option<i64> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("file_id")?.parse().ok()
}
