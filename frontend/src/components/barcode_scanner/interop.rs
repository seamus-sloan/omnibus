//! Scanner interop: pure JS-string builders plus the `dioxus::document::eval`
//! seam that mounts the vendored zxing decoder. One implementation serves both
//! interactive targets — the `window.__omnibusOnScan*` shims forward into
//! `dioxus.send(...)` (drained via `Eval::recv`), which web and mobile both
//! support, unlike the reader's web-only `wasm_bindgen` callbacks.

// SSR never evals: it compiles the builders only so their tests run under the
// `server` feature (the matrix `cargo test -p omnibus-frontend` uses).
#![cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]

#[cfg(any(feature = "web", feature = "mobile"))]
use dioxus::document::Eval;

/// Absolute URLs of the vendored scanner runtime, resolved from the `asset!`
/// bundle. `decoder` (the zxing IIFE) must evaluate before `glue`, which binds
/// `window.ZXingWASM` at start time; `wasm` is passed through to the glue so
/// zxing loads the bundled binary instead of its jsDelivr default.
pub(super) struct ScannerScripts {
    pub decoder: String,
    pub glue: String,
    pub wasm: String,
}

/// JS→Rust scanner events, forwarded by the shims the install script defines.
#[derive(serde::Deserialize)]
#[serde(tag = "kind")]
pub(super) enum ScannerEvent {
    /// A confirmed EAN-13 decode. The glue stops the loop before sending.
    Result { text: String },
    /// One of "starting" / "ready" / "denied" / "unsupported" / "error".
    Status { state: String },
    /// "1" once the active camera track advertises a torch capability.
    Torch { flag: String },
}

/// Build the install IIFE: define the `__omnibusOnScan*` → `dioxus.send` shims,
/// then load the vendored runtime **in order** (zxing → glue) and start the
/// camera. Scripts are chained through `onload` promises rather than emitted as
/// `<script>` tags, so the same script works on mobile (no SSR ordering) and on
/// web without adding markup that SSR would have to match. Each script is
/// skipped when its global already exists (re-entry after "Enter ISBN
/// instead"), and each load has a 10 s timeout so a stalled request surfaces
/// the error state instead of a permanently black viewfinder. `start`'s own
/// promise is returned into this chain too, so a decoder-init failure it
/// already reports via `status()` also reaches this outer `.catch`.
pub(super) fn install_scanner_js(video_id: &str, scripts: &ScannerScripts) -> String {
    let video_lit = js_str(video_id);
    let decoder = js_str(&scripts.decoder);
    let glue = js_str(&scripts.glue);
    let wasm = js_str(&scripts.wasm);
    format!(
        r#"(function(){{
  window.__omnibusOnScanResult=function(t){{dioxus.send({{kind:"Result",text:t}});}};
  window.__omnibusOnScanStatus=function(s){{dioxus.send({{kind:"Status",state:s}});}};
  window.__omnibusOnScanTorch=function(f){{dioxus.send({{kind:"Torch",flag:f}});}};
  function load(src){{return new Promise(function(res,rej){{
    var s=document.createElement("script");s.src=src;s.async=false;
    var t=setTimeout(function(){{rej();}},10000);
    s.onload=function(){{clearTimeout(t);res();}};
    s.onerror=function(){{clearTimeout(t);rej();}};
    document.head.appendChild(s);
  }});}}
  function ensure(has,src){{return has()?Promise.resolve():load(src);}}
  ensure(function(){{return !!window.ZXingWASM;}},{decoder})
    .then(function(){{return ensure(function(){{return !!window.OmnibusScanner;}},{glue});}})
    .then(function(){{
      return window.OmnibusScanner.start({video_lit},{{wasmUrl:{wasm}}});
    }})
    .catch(function(){{dioxus.send({{kind:"Status",state:"error"}});}});
}})();"#
    )
}

/// Build the teardown IIFE — stops the decode loop and releases the camera
/// track so the OS capture indicator goes out when the scanner unmounts.
pub(super) fn stop_scanner_js() -> String {
    "window.OmnibusScanner && window.OmnibusScanner.stop();".to_string()
}

/// Build the torch IIFE for the given state.
pub(super) fn set_torch_js(on: bool) -> String {
    format!("window.OmnibusScanner && window.OmnibusScanner.setTorch({on});")
}

/// JSON-quote `s` for embedding as a JS literal, falling back to an empty
/// string literal (rather than [`crate::js_interop::json_literal`]'s `null`)
/// since every caller here interpolates directly into a JS string-typed
/// argument position.
fn js_str(s: &str) -> String {
    crate::js_interop::json_literal_or(s, "\"\"")
}

/// Eval the install script and return the persistent [`Eval`] the caller drains
/// for scanner events.
#[cfg(any(feature = "web", feature = "mobile"))]
pub(super) fn install_scanner_surface(video_id: &str, scripts: &ScannerScripts) -> Eval {
    dioxus::document::eval(&install_scanner_js(video_id, scripts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts() -> ScannerScripts {
        ScannerScripts {
            decoder: "/assets/zxing-reader.js".into(),
            glue: "/assets/barcode-scanner-glue.js".into(),
            wasm: "/assets/zxing_reader.wasm".into(),
        }
    }

    #[test]
    fn install_scanner_js_defines_shims_and_starts_the_named_video() {
        let js = install_scanner_js("omnibus-scanner-video", &scripts());
        assert!(js.contains("window.__omnibusOnScanResult"));
        assert!(js.contains("window.__omnibusOnScanStatus"));
        assert!(js.contains("window.__omnibusOnScanTorch"));
        assert!(js.contains(r#"OmnibusScanner.start("omnibus-scanner-video""#));
    }

    #[test]
    fn install_scanner_js_loads_the_decoder_before_the_glue_and_passes_the_wasm_url() {
        let js = install_scanner_js("v", &scripts());
        let decoder = js.find("/assets/zxing-reader.js").expect("decoder url");
        let glue = js
            .find("/assets/barcode-scanner-glue.js")
            .expect("glue url");
        assert!(decoder < glue, "zxing must be loaded before the glue");
        assert!(js.contains(r#"wasmUrl:"/assets/zxing_reader.wasm""#));
    }

    #[test]
    fn install_scanner_js_reports_an_error_when_a_script_fails_to_load() {
        let js = install_scanner_js("v", &scripts());
        assert!(js.contains(r#".catch(function(){dioxus.send({kind:"Status",state:"error"});})"#));
    }

    #[test]
    fn scanner_event_deserializes_each_kind() {
        let ev: ScannerEvent = serde_json::from_str(r#"{"kind":"Result","text":"978"}"#).unwrap();
        assert!(matches!(ev, ScannerEvent::Result { text } if text == "978"));
        let ev: ScannerEvent =
            serde_json::from_str(r#"{"kind":"Status","state":"denied"}"#).unwrap();
        assert!(matches!(ev, ScannerEvent::Status { state } if state == "denied"));
        let ev: ScannerEvent = serde_json::from_str(r#"{"kind":"Torch","flag":"1"}"#).unwrap();
        assert!(matches!(ev, ScannerEvent::Torch { flag } if flag == "1"));
    }

    #[test]
    fn torch_and_stop_builders_target_the_glue_surface() {
        assert!(set_torch_js(true).contains("setTorch(true)"));
        assert!(set_torch_js(false).contains("setTorch(false)"));
        assert!(stop_scanner_js().contains("OmnibusScanner.stop()"));
    }
}
