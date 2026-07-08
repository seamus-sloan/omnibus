//! Mobile reader interop — mounts the vendored epub.js in the wry WebView and
//! bridges its events into the shared reader signals.
//!
//! Mobile compiles the same reader chrome + overlays as web, but runs natively
//! (not WASM), so the web `web_sys`/`wasm_bindgen` callback registration can't
//! be reused. This installer instead evals a script that forwards the glue's
//! `window.__omnibusOn*` calls into `dioxus.send(...)` and drains them with
//! `Eval::recv().await`, mirroring `listen/mobile`. Reading position and saved
//! highlights are synced through the mobile REST [`crate::data`] layer.

#![cfg(feature = "mobile")]

use dioxus::prelude::*;

use omnibus_shared::{Highlight, ProgressFormat, ProgressUpdate};

use crate::data;

use super::prefs::ReaderPrefs;
use super::search_panel::SearchResult;
use super::selection::SelectionData;
use super::toc_drawer::TocEntry;
use super::{reader_call_json, reader_call_json2, ReaderStatus, RelocateData};

mod interop;

/// The signals the reader drives from the JS event channel — the mobile mirror
/// of the web `interop::InteropSignals`.
#[derive(Copy, Clone)]
pub(super) struct InteropSignals {
    pub status: Signal<ReaderStatus>,
    pub loc: Signal<RelocateData>,
    pub selection: Signal<Option<SelectionData>>,
    pub highlights: Signal<Vec<Highlight>>,
    pub toc: Signal<Vec<TocEntry>>,
    pub search_results: Signal<Vec<SearchResult>>,
}

/// JS→Rust reader events, forwarded by the shims the install script defines in
/// the WebView. The `json` payloads are the glue's own `JSON.stringify`ed
/// structs, parsed into the reader types on receipt (same shapes web parses).
#[derive(serde::Deserialize)]
#[serde(tag = "kind")]
enum ReaderEvent {
    Relocate { json: String },
    Status { state: String },
    Selection { json: String },
    Toc { json: String },
    Search { json: String },
}

/// Install the mobile reader interop: on every `uuid` change, mount epub.js
/// against the tokened file URL and drain its event channel into the reader
/// signals; separately, re-apply the theme whenever it changes. Runs as
/// unconditional hooks (rule 07) from `BookReadPage`'s `use_reader_signals`.
pub(super) fn install_reader_mobile_interop(
    uuid: String,
    prefs: ReaderPrefs,
    sigs: InteropSignals,
    server_url: String,
) {
    let mut status = sigs.status;
    use_effect(use_reactive!(|(uuid, server_url)| {
        status.set(ReaderStatus::Loading);
        spawn(mount_and_drain(
            uuid.clone(),
            prefs,
            sigs,
            server_url.clone(),
        ));
    }));

    // Theme is app-wide (not pushed by the prefs setter), so mirror it into the
    // glue whenever it changes — the web interop does the same.
    let theme = prefs.theme;
    use_effect(move || {
        reader_call_json("setTheme", theme.read().as_attr());
    });
}

/// Resolve the resume CFI, mount epub.js, then drain its events forever. The
/// starting position is server-authoritative (falling back to the local
/// in-memory cache), matching the web bootstrap.
async fn mount_and_drain(
    uuid: String,
    prefs: ReaderPrefs,
    sigs: InteropSignals,
    server_url: String,
) {
    let local_saved = crate::reader_progress::load(&uuid);
    let server_cfi = data::get_progress(&server_url, &uuid, ProgressFormat::Epub)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.epub_cfi);
    let cfi = server_cfi.or(local_saved);

    let token = data::token_store::get();
    let file_url = interop::file_token_url(&server_url, &uuid, token.as_deref());
    let eval = interop::install_reader_surface(&file_url, &init_opts(prefs, cfi));

    // Fetch the saved highlights up front; they're replayed into the viewer
    // once the glue reports `ready` (annotations need a live rendition).
    let saved = data::list_highlights(&server_url, &uuid)
        .await
        .unwrap_or_default();

    drain_reader_events(eval, uuid, server_url, sigs, saved).await;
}

/// Build the glue `init` options bag from the current reader prefs and the
/// resume CFI. Field names match the vendored glue's `opts` keys.
fn init_opts(prefs: ReaderPrefs, cfi: Option<String>) -> serde_json::Value {
    serde_json::json!({
        "cfi": cfi,
        "fontSize": *prefs.font_size.peek(),
        "theme": prefs.theme.peek().as_attr(),
        "fontFamily": prefs.typeface.peek().to_css(),
        "lineHeight": prefs.line_spacing.peek().to_css(),
        "maxWidth": prefs.margins.peek().to_css(),
        "justify": *prefs.justify.peek(),
        "spread": prefs.spread.peek().to_css(),
    })
}

/// Drain the JS→Rust reader event channel forever, updating the reader signals
/// and persisting position on relocate. Returns when the channel closes
/// (surface torn down / navigation).
async fn drain_reader_events(
    mut eval: dioxus::document::Eval,
    uuid: String,
    server_url: String,
    sigs: InteropSignals,
    saved_highlights: Vec<Highlight>,
) {
    let InteropSignals {
        mut status,
        mut loc,
        mut selection,
        mut highlights,
        mut toc,
        mut search_results,
    } = sigs;
    let mut last_cfi: Option<String> = None;
    loop {
        match eval.recv::<ReaderEvent>().await {
            Ok(ReaderEvent::Relocate { json }) => {
                if let Ok(data) = serde_json::from_str::<RelocateData>(&json) {
                    if let Some(cfi) = data.cfi.clone() {
                        crate::reader_progress::save(&uuid, &cfi);
                        // Only POST on an actual position change (the glue
                        // debounces, but re-renders can re-emit the same CFI).
                        if last_cfi.as_deref() != Some(cfi.as_str()) {
                            last_cfi = Some(cfi.clone());
                            persist_progress(&uuid, &server_url, cfi);
                        }
                    }
                    loc.set(data);
                }
            }
            Ok(ReaderEvent::Status { state }) => {
                let st = match state.as_str() {
                    "ready" => ReaderStatus::Ready,
                    "error" => ReaderStatus::Failed,
                    _ => ReaderStatus::Loading,
                };
                status.set(st);
                // Replay saved highlights into the freshly-mounted rendition.
                if matches!(st, ReaderStatus::Ready) && highlights.peek().is_empty() {
                    for h in &saved_highlights {
                        reader_call_json2("addAnnotation", &h.epub_cfi_range, h.color.as_str());
                    }
                    highlights.set(saved_highlights.clone());
                }
            }
            Ok(ReaderEvent::Selection { json }) => {
                if let Ok(data) = serde_json::from_str::<SelectionData>(&json) {
                    selection.set(Some(data));
                }
            }
            Ok(ReaderEvent::Toc { json }) => {
                if let Ok(entries) = serde_json::from_str::<Vec<TocEntry>>(&json) {
                    toc.set(entries);
                }
            }
            Ok(ReaderEvent::Search { json }) => {
                if let Ok(rs) = serde_json::from_str::<Vec<SearchResult>>(&json) {
                    search_results.set(rs);
                }
            }
            Err(_) => return,
        }
    }
}

/// Persist the latest CFI to the server (fire-and-forget); the local in-memory
/// save already happened on the calling side.
fn persist_progress(uuid: &str, server_url: &str, cfi: String) {
    let uuid = uuid.to_string();
    let server_url = server_url.to_string();
    spawn(async move {
        let update = ProgressUpdate {
            book_uuid: uuid,
            format: ProgressFormat::Epub,
            epub_cfi: Some(cfi),
            audio_position_seconds: None,
        };
        let _ = data::save_progress(&server_url, update).await;
    });
}
