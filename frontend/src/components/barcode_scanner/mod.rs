//! Reusable camera barcode scanner: rear-camera preview, auto-detecting
//! EAN-13 decode (no shutter), optional torch, and a permanent escape hatch to
//! manual entry. Decoding uses the vendored zxing WASM in `assets/vendor/`,
//! never a CDN — mobile bundles its assets. The rsx is target-agnostic (rule
//! 07); only the post-mount effect that opens the camera is target-gated.

mod interop;
#[cfg(test)]
mod tests;

use dioxus::prelude::*;

// The vendored runtime is loaded from JS by `install_scanner`, so SSR — which
// never opens a camera — sees these consts (and `from_glue`) as unused.
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
const ZXING_JS: Asset = asset!("/assets/vendor/zxing-reader.min.js");
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
const ZXING_WASM: Asset = asset!("/assets/vendor/zxing_reader.wasm");
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
const SCANNER_GLUE_JS: Asset = asset!("/assets/vendor/barcode-scanner-glue.js");

/// The single `<video>` the glue attaches the camera stream to. A constant
/// rather than a prop: only one scanner is ever mounted at a time, and the
/// glue looks the element up by id from JS.
const VIDEO_ID: &str = "omnibus-scanner-video";

/// Where the camera got to. Every variant except [`ScanStatus::Ready`] renders
/// its own copy under the viewfinder; the two terminal failures also surface
/// the manual-entry escape hatch as the primary action.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum ScanStatus {
    /// Pre-mount and while `getUserMedia` is resolving. The SSR value, so the
    /// server's HTML and the first client paint agree.
    Starting,
    /// Camera is live and the decode loop is running.
    Ready,
    /// The user (or the OS) refused camera access.
    Denied,
    /// No camera, or a WebView/origin without `getUserMedia` at all.
    Unsupported,
    /// Anything else — a failed script load, a decoder that never started.
    Error,
}

impl ScanStatus {
    /// Parse the glue's status string; anything unrecognized is an error
    /// rather than a silently-ignored state.
    #[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
    fn from_glue(state: &str) -> Self {
        match state {
            "starting" => Self::Starting,
            "ready" => Self::Ready,
            "denied" => Self::Denied,
            "unsupported" => Self::Unsupported,
            _ => Self::Error,
        }
    }

    /// Whether the camera is out of the running for good, so the screen should
    /// lead with manual entry instead of a dead viewfinder.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Denied | Self::Unsupported | Self::Error)
    }

    /// The line shown under (or in place of) the viewfinder.
    fn message(self) -> &'static str {
        match self {
            Self::Starting => "Starting camera\u{2026}",
            Self::Ready => "Point the camera at the barcode on the back cover.",
            Self::Denied => {
                "Omnibus can't use the camera. Allow camera access in your settings, or type the ISBN instead."
            }
            Self::Unsupported => {
                "No camera is available on this device. Type the ISBN instead."
            }
            Self::Error => "The scanner couldn't start. Type the ISBN instead.",
        }
    }
}

/// Live camera scanner. Calls `on_detect` once with a confirmed 13-digit EAN
/// (the glue stops the decode loop before reporting), and `on_manual` when the
/// reader chooses the keypad — either by asking for it or because the camera
/// is unavailable.
#[component]
pub fn BarcodeScanner(on_detect: EventHandler<String>, on_manual: EventHandler<()>) -> Element {
    let status = use_signal(|| ScanStatus::Starting);
    let torch_available = use_signal(|| false);
    let mut torch_on = use_signal(|| false);

    install_scanner(status, torch_available, on_detect);

    rsx! {
        div { class: "scanner", "data-testid": "barcode-scanner",
            div {
                class: "scanner-stage",
                "data-state": scanner_state_attr(status()),
                video {
                    id: "{VIDEO_ID}",
                    class: "scanner-video",
                    "data-testid": "barcode-scanner-video",
                    autoplay: true,
                    muted: true,
                    playsinline: true,
                }
                div { class: "scanner-reticle",
                    span { class: "scanner-corner scanner-corner--tl" }
                    span { class: "scanner-corner scanner-corner--tr" }
                    span { class: "scanner-corner scanner-corner--bl" }
                    span { class: "scanner-corner scanner-corner--br" }
                    span { class: "scanner-sweep" }
                }
            }
            p {
                class: "scanner-status",
                "data-testid": "barcode-scanner-status",
                role: "status",
                "{status().message()}"
            }
            div { class: "scanner-actions",
                if torch_available() && !status().is_terminal() {
                    button {
                        r#type: "button",
                        class: "btn ghost scanner-torch",
                        "data-testid": "barcode-scanner-torch",
                        "aria-pressed": "{torch_on()}",
                        onclick: move |_| {
                            let next = !torch_on();
                            torch_on.set(next);
                            set_torch(next);
                        },
                        if torch_on() { "Torch on" } else { "Torch off" }
                    }
                }
                button {
                    r#type: "button",
                    class: if status().is_terminal() { "btn primary" } else { "btn ghost" },
                    "data-testid": "barcode-scanner-manual",
                    onclick: move |_| on_manual.call(()),
                    "Enter ISBN instead"
                }
            }
        }
    }
}

/// The `data-state` the stage carries, so CSS can drop the (permanently blank)
/// viewfinder once the camera is definitively out.
fn scanner_state_attr(status: ScanStatus) -> &'static str {
    match status {
        ScanStatus::Starting => "starting",
        ScanStatus::Ready => "ready",
        _ => "off",
    }
}

/// Mount the camera and drain the glue's event channel. Declared as
/// unconditional hooks on every target (rule 07) — the body is what's gated,
/// so SSR and the first client paint render the same tree.
#[allow(unused_variables)]
fn install_scanner(
    status: Signal<ScanStatus>,
    torch_available: Signal<bool>,
    on_detect: EventHandler<String>,
) {
    #[cfg(any(feature = "web", feature = "mobile"))]
    {
        use dioxus::core::Task;

        let mut status = status;
        let mut torch_available = torch_available;
        // Retain the drain task so an unmount cancels the long-lived
        // `Eval::recv` loop rather than leaking it behind a dead camera.
        let mut task = use_signal(|| None::<Task>);
        use_effect(move || {
            if let Some(prev) = task.write().take() {
                prev.cancel();
            }
            let scripts = interop::ScannerScripts {
                decoder: ZXING_JS.to_string(),
                glue: SCANNER_GLUE_JS.to_string(),
                wasm: ZXING_WASM.to_string(),
            };
            let mut eval = interop::install_scanner_surface(VIDEO_ID, &scripts);
            task.set(Some(spawn(async move {
                while let Ok(event) = eval.recv::<interop::ScannerEvent>().await {
                    match event {
                        interop::ScannerEvent::Result { text } => on_detect.call(text),
                        interop::ScannerEvent::Status { state } => {
                            status.set(ScanStatus::from_glue(&state))
                        }
                        interop::ScannerEvent::Torch { flag } => torch_available.set(flag == "1"),
                    }
                }
            })));
        });

        use_drop(move || {
            if let Some(prev) = task.write().take() {
                prev.cancel();
            }
            let _ = dioxus::document::eval(&interop::stop_scanner_js());
        });
    }
}

/// Push the torch state into the glue. No-op where there's no WebView to talk
/// to (SSR, tests).
#[allow(unused_variables)]
fn set_torch(on: bool) {
    #[cfg(any(feature = "web", feature = "mobile"))]
    {
        let _ = dioxus::document::eval(&interop::set_torch_js(on));
    }
}
