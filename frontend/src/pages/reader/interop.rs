//! Web-only JS interop for the reader: registers `__omnibusOn*` callbacks
//! on `window`, primes the epub.js viewer via the bootstrap IIFE, and
//! seeds the annotation layer from the saved highlights. Extracted from
//! `BookReadPage` so the parent reads as plain Rust signal/state wiring.

use dioxus::prelude::*;
use omnibus_shared::Highlight;

use super::prefs::ReaderPrefs;
use super::search_panel::SearchResult;
use super::selection::SelectionData;
use super::signals::resolve_chapter_position;
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
    pub chrome_hidden: Signal<bool>,
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
    use super::reader_call_json;
    use crate::js_interop::json_literal;

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
        let deep_link_cfi = parse_cfi_from_url();
        let size = *prefs.font_size.read();
        let theme_name = theme.read().as_attr();
        let file_url = match parse_file_id_from_url() {
            Some(fid) => format!("/api/ebooks/{uuid}/file?file_id={fid}"),
            None => format!("/api/ebooks/{uuid}/file"),
        };
        let url_lit = json_literal(&file_url);
        let theme_lit = json_literal(theme_name);
        let font_family_lit = json_literal(prefs.typeface.read().to_css());
        let line_height_lit = json_literal(prefs.line_spacing.read().to_css());
        let max_width_lit = json_literal(prefs.margins.read().to_css());
        let justify_val = *prefs.justify.read();
        let spread_lit = json_literal(prefs.spread.read().to_css());

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
            deep_link_cfi,
            bootstrap,
            highlights,
            sigs.loc,
        ));
    }));

    use_effect(move || {
        reader_call_json("setTheme", theme.read().as_attr());
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
        mut chrome_hidden,
        ..
    } = sigs;

    let uuid_for_save = uuid_cb;
    // Last SEEN position (echoes included), mirroring the mobile reader:
    // a later non-echo re-emit of the same CFI (a layout re-render) stays
    // deduped rather than re-posting — and re-clocking — the unmoved
    // position.
    let mut last_seen_cfi: Option<String> = None;
    let relocate = Closure::<dyn FnMut(String)>::new(move |json: String| {
        if let Ok(mut data) = serde_json::from_str::<super::RelocateData>(&json) {
            // Re-derive chapter/total from the TOC's own order rather than
            // trusting the glue's numbers verbatim — see
            // `signals::resolve_chapter_position` (issue #1909, AC1).
            let toc_snapshot = toc.peek().clone();
            let previous_chapter = loc.peek().chapter;
            let (chapter, total_chapters) =
                resolve_chapter_position(&toc_snapshot, &data, previous_chapter);
            data.chapter = chapter;
            data.total_chapters = total_chapters;

            // Echo emissions re-state a position the server already holds —
            // persisting one would stamp a fresh clock on an unmoved
            // position and shadow a newer audiobook spot at the
            // cross-format clock gate. Render, don't write. The same-CFI
            // dedup mirrors the mobile reader: layout re-emissions (window
            // resize, Aa panel changes) re-fire `relocated` at the current
            // CFI, and a re-post would re-clock the unmoved position too.
            let moved = match (&data.cfi, &last_seen_cfi) {
                (Some(new), Some(seen)) => new != seen,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if data.cfi.is_some() {
                last_seen_cfi = data.cfi.clone();
            }
            if let Some(cfi) = data.cfi.clone().filter(|_| !data.echo && moved) {
                crate::reader_progress::save(&uuid_for_save, &cfi);
                let uuid_for_post = uuid_for_save.clone();
                let cfi_for_post = cfi;
                // `progress_percent` is the same whole-book figure the
                // footer/ribbon render (`data.pct`) — sending it keeps the
                // landing hero's stored percent in step with what the
                // reader itself is showing (issue #1909, AC2).
                let percent_for_post = i64::from(data.pct.min(100));
                wasm_bindgen_futures::spawn_local(async move {
                    // `client_updated_at` is the event time the server's
                    // conditional upsert arbitrates on; without it a write
                    // degrades to receipt-time last-write-wins (#1864).
                    let body = serde_json::json!({
                        "update": {
                            "book_uuid": uuid_for_post,
                            "format": "epub",
                            "epub_cfi": cfi_for_post,
                            "progress_percent": percent_for_post,
                            "client_updated_at": crate::time::now_unix(),
                        }
                    });
                    if let Ok(req) = gloo_net::http::Request::post("/api/rpc/progress").json(&body)
                    {
                        let _ = req.send().await;
                    }
                });
            }
            // A relocate always names the (corrected) current position, so
            // it also signals that an in-flight chapter jump has finished
            // rendering — flip a TOC-jump-triggered `Loading` back to
            // `Ready` (issue #1909, AC3). Guarded so a relocate stray after
            // a load `Failed` can't resurrect the overlay as if nothing
            // went wrong.
            if *status.peek() == ReaderStatus::Loading {
                status.set(ReaderStatus::Ready);
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
    // Glue reports the selection collapsed (tap-away / handle drag ended
    // empty): dismiss the popover. On phone widths this is the only dismiss
    // path — the click-scrim is hidden there so native selection-handle
    // drags reach the prose.
    let on_selection_cleared = Closure::<dyn FnMut(String)>::new(move |_: String| {
        selection.set(None);
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
    // Centre-tap toggle from the glue: flip the chrome-visibility signal (the
    // arg is unused — the glue calls it with "").
    let on_toggle_chrome = Closure::<dyn FnMut(String)>::new(move |_: String| {
        chrome_hidden.toggle();
    });
    for (name, cb) in [
        ("__omnibusOnRelocate", &relocate),
        ("__omnibusOnStatus", &on_status),
        ("__omnibusOnSelection", &on_selection),
        ("__omnibusOnSelectionCleared", &on_selection_cleared),
        ("__omnibusOnToc", &on_toc),
        ("__omnibusOnSearchResults", &on_search),
        ("__omnibusOnToggleChrome", &on_toggle_chrome),
    ] {
        let _ = js_sys::Reflect::set(
            window,
            &JsValue::from_str(name),
            cb.as_ref().unchecked_ref(),
        );
    }
    vec![
        relocate,
        on_status,
        on_selection,
        on_selection_cleared,
        on_toc,
        on_search,
        on_toggle_chrome,
    ]
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

/// Resolve the starting CFI (a `?cfi=` deep link, else server progress, else
/// the local save), run the epub.js bootstrap IIFE, then seed the annotation
/// layer from the saved highlights and publish them into the `highlights`
/// signal.
///
/// `deep_link_cfi` wins outright: the reader was opened *at* a passage (from
/// the book-detail saved-passages list), so resuming where this book was last
/// left off would ignore the whole point of the link. The relocate handler
/// then persists the new position as usual.
///
/// The same progress fetch also seeds `loc` with the server's stored
/// whole-book percent (flagged approximate) so the footer/ribbon show the
/// last-known position immediately — not a blank (or a frozen 0%) while
/// epub.js fetches, parses, and paginates the book (issue #1896, AC2). The
/// first real relocate overwrites the seed wholesale.
#[cfg(feature = "web")]
async fn spawn_bootstrap_and_highlights(
    uuid: String,
    local_saved: Option<String>,
    deep_link_cfi: Option<String>,
    lits: BootstrapLiterals,
    mut highlights: Signal<Vec<Highlight>>,
    mut loc: Signal<super::RelocateData>,
) {
    use super::bootstrap::{reader_bootstrap_js, BootstrapArgs};
    use super::reader_call_json2;
    use crate::data;
    use crate::js_interop::json_literal;

    // Only pay for the progress round trip when nothing already decided.
    let chosen = match deep_link_cfi {
        Some(cfi) => Some(cfi),
        None => {
            let record = crate::data::get_progress("", &uuid, omnibus_shared::ProgressFormat::Epub)
                .await
                .ok()
                .flatten();
            let stored_pct = record.as_ref().and_then(|r| r.progress_percent);
            if let Some(pct) = stored_pct.filter(|p| (1..=100).contains(p)) {
                // Guarded on "no relocate yet" so a fast epub.js mount can
                // never be clobbered by this slower round trip.
                if loc.peek().cfi.is_none() {
                    loc.set(super::RelocateData {
                        pct: pct as u32,
                        pct_approx: true,
                        ..Default::default()
                    });
                }
            }
            record.and_then(|r| r.epub_cfi).or(local_saved)
        }
    };
    let cfi_arg = json_literal(&chosen);
    let locations_key_lit = json_literal(&uuid);
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
        locations_key_lit: &locations_key_lit,
    });
    let _ = dioxus::document::eval(&js);

    if let Ok(list) = data::list_highlights("", &uuid).await {
        for h in &list {
            // Kobo-origin highlights carry no CFI — nothing to paint.
            if let Some(cfi) = &h.epub_cfi_range {
                reader_call_json2("addAnnotation", cfi, h.color.as_str());
            }
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

/// Extract a `?cfi=` deep link from the current URL — the book-detail
/// saved-passages list opens the reader at a specific highlight this way.
/// Blank values are treated as absent so `?cfi=` alone falls back to resume.
#[cfg(feature = "web")]
fn parse_cfi_from_url() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    let cfi = params.get("cfi")?;
    (!cfi.trim().is_empty()).then_some(cfi)
}
