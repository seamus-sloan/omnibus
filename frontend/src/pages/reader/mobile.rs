//! Mobile reader interop: mounts the vendored epub.js in the wry WebView and
//! bridges the glue's `window.__omnibusOn*` callbacks into Rust over the Dioxus
//! `Eval` channel (mobile is native, not WASM, so the web `wasm_bindgen` path
//! can't be reused). Position + saved highlights sync through the mobile REST
//! [`crate::data`] layer.

#![cfg(feature = "mobile")]

use dioxus::core::Task;
use dioxus::prelude::*;

use omnibus_shared::{Highlight, ProgressFormat, ProgressUpdate};

use crate::data;

use super::prefs::ReaderPrefs;
use super::search_panel::SearchResult;
use super::selection::SelectionData;
use super::toc_drawer::TocEntry;
use super::{reader_call_json, reader_call_json2, ReaderStatus, RelocateData};

mod interop;
pub(super) mod prefs_storage;

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
    pub chrome_hidden: Signal<bool>,
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
    SelectionCleared,
    ShareText { text: String },
    ShareImage { json: String },
    Toc { json: String },
    Search { json: String },
    ToggleChrome,
}

/// Payload of a [`ReaderEvent::ShareImage`] — the glue's rendered quote card.
#[derive(serde::Deserialize)]
struct ShareImagePayload {
    name: String,
    #[serde(rename = "dataUrl")]
    data_url: String,
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
    // Retain the mount/drain task so a uuid / server-url change cancels the
    // prior long-lived `Eval::recv` loop instead of leaking it (a plain
    // `spawn` per effect run would stack an orphaned drain loop each time).
    let mut mount_task = use_signal(|| None::<Task>);
    use_effect(use_reactive!(|(uuid, server_url)| {
        if let Some(prev) = mount_task.write().take() {
            prev.cancel();
        }
        status.set(ReaderStatus::Loading);
        let task = spawn(mount_and_drain(
            uuid.clone(),
            prefs,
            sigs,
            server_url.clone(),
        ));
        mount_task.set(Some(task));
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
/// in-memory cache), matching the web bootstrap. Persisted typography prefs
/// are loaded and applied to `prefs`'s signals first, so the glue's init
/// options (built from those signals just below) mount with the restored
/// values rather than the target-agnostic defaults `init_reader_prefs`
/// seeded at signal-construction time.
async fn mount_and_drain(
    uuid: String,
    prefs: ReaderPrefs,
    sigs: InteropSignals,
    server_url: String,
) {
    prefs_storage::load_and_apply_reader_prefs(prefs).await;
    let local_saved = crate::reader_progress::load(&uuid);
    let server_cfi = data::get_progress(&server_url, &uuid, ProgressFormat::Epub)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.epub_cfi);
    let cfi = server_cfi.or(local_saved);

    // Backstop for the route-level offline bounce (`BookReadPage`'s guard):
    // at cold start the app may still believe it's online, and the progress
    // fetch above is the bounded attempt that flips NetState. Offline with
    // no local EPUB, mounting would leave epub.js hanging on "Loading…" —
    // fail fast, and force the chrome visible: the glue that relays the
    // tap-to-summon-chrome event never mounts on this path, so a hidden top
    // bar would strand the user with no way back.
    if crate::offline::sync::is_offline()
        && crate::offline::downloads::local_epub_url(&uuid).is_none()
    {
        let mut status = sigs.status;
        let mut chrome_hidden = sigs.chrome_hidden;
        status.set(ReaderStatus::Offline);
        chrome_hidden.set(false);
        return;
    }

    let token = data::token_store::get();
    // A completed offline download serves the EPUB from the loopback media
    // server; otherwise stream it from the real server with `?token=`.
    let file_url = crate::offline::downloads::local_epub_url(&uuid)
        .unwrap_or_else(|| interop::file_token_url(&server_url, &uuid, token.as_deref()));
    // Resolve the vendored runtime URLs from the `asset!` bundle so the WebView
    // loads them JSZip-first (see `install_surface_js`).
    let scripts = interop::ReaderScripts {
        jszip: super::JSZIP_JS.to_string(),
        epub: super::EPUBJS_JS.to_string(),
        glue: super::READER_GLUE_JS.to_string(),
    };
    let eval = interop::install_reader_surface(
        &file_url,
        &init_opts(&uuid, prefs, cfi),
        &scripts,
        crate::native_share::supported(),
    );

    // Fetch the saved highlights up front; they're replayed into the viewer
    // once the glue reports `ready` (annotations need a live rendition).
    let saved = data::list_highlights(&server_url, &uuid)
        .await
        .unwrap_or_default();

    drain_reader_events(eval, uuid, server_url, sigs, saved).await;
}

/// Build the glue `init` options bag from the current reader prefs and the
/// resume CFI. Field names match the vendored glue's `opts` keys.
fn init_opts(uuid: &str, prefs: ReaderPrefs, cfi: Option<String>) -> serde_json::Value {
    serde_json::json!({
        "cfi": cfi,
        "fontSize": *prefs.font_size.peek(),
        "theme": prefs.theme.peek().as_attr(),
        "fontFamily": prefs.typeface.peek().to_css(),
        "lineHeight": prefs.line_spacing.peek().to_css(),
        "maxWidth": prefs.margins.peek().to_css(),
        "justify": *prefs.justify.peek(),
        "spread": prefs.spread.peek().to_css(),
        // Per-book locations-cache key: page numbers appear instantly on
        // re-opens instead of waiting out the WebView's slow whole-book
        // pagination pass (see the glue's locations cache).
        "locationsKey": uuid,
        // WKWebView dispatches no events to listeners inside a sandboxed
        // iframe without `allow-scripts` — swipe/tap page turns and the
        // `selected` relay never fire without this. Mobile-only; the web
        // build keeps the stricter sandbox.
        "allowScriptedContent": true,
    })
}

/// Drain the JS→Rust reader event channel forever, updating the reader signals
/// and persisting position on relocate. Returns when the channel closes
/// (surface torn down / navigation). The drain loop itself is
/// [`crate::js_interop::drain_events`], shared with the barcode scanner and
/// mobile audio interop.
async fn drain_reader_events(
    eval: dioxus::document::Eval,
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
        mut chrome_hidden,
    } = sigs;
    let mut last_cfi: Option<String> = None;
    crate::js_interop::drain_events(eval, move |event: ReaderEvent| match event {
        ReaderEvent::Relocate { json } => {
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
        ReaderEvent::Status { state } => {
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
        ReaderEvent::Selection { json } => {
            if let Ok(data) = serde_json::from_str::<SelectionData>(&json) {
                selection.set(Some(data));
            }
        }
        // The selection collapsed (tap-away / adjusted to nothing): dismiss
        // the popover. This is the only dismiss path on mobile — no scrim
        // overlays the prose, so native handle drags get through.
        ReaderEvent::SelectionCleared => selection.set(None),
        // Native share bridge: the glue routes Share actions here (instead of
        // the Web Share API, absent in WKWebView) and iOS presents the real
        // sheet. Shims are only installed when `supported()`.
        ReaderEvent::ShareText { text } => crate::native_share::share_text(&text),
        ReaderEvent::ShareImage { json } => {
            if let Ok(p) = serde_json::from_str::<ShareImagePayload>(&json) {
                crate::native_share::share_png_data_url(&p.name, &p.data_url);
            }
        }
        ReaderEvent::Toc { json } => {
            if let Ok(entries) = serde_json::from_str::<Vec<TocEntry>>(&json) {
                toc.set(entries);
            }
        }
        ReaderEvent::Search { json } => {
            if let Ok(rs) = serde_json::from_str::<Vec<SearchResult>>(&json) {
                search_results.set(rs);
            }
        }
        // Centre tap: flip chrome visibility. The signal is the single source
        // of truth (the glue signals rather than mutating the Dioxus-owned
        // bar DOM directly); the class drives the CSS.
        ReaderEvent::ToggleChrome => chrome_hidden.toggle(),
    })
    .await;
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
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(now_unix()),
        };
        let _ = data::save_progress(&server_url, update).await;
    });
}

/// Current unix time in seconds from the device clock — stamped onto every
/// `ProgressUpdate` so most-recent-wins resolves on client event time
/// rather than server receipt time (issue #1362). This module only
/// compiles under `feature = "mobile"` (native, not wasm32), so the
/// platform clock is always `std::time::SystemTime`.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
