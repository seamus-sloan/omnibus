//! Hero "Export" dropdown — condenses the per-device send/download actions
//! (Download EPUB, Download audiobook, Send to Kindle, Send to Kobo) behind
//! one trigger so the hero CTA row stays a single primary action plus this
//! menu. Web/server only; the book-detail hero isn't compiled on mobile.

use dioxus::prelude::*;

use crate::components::SendToKindleButton;

/// The "Export" trigger + dropdown panel. Renders the download links for
/// whichever formats the book has, the interactive Send-to-Kindle button,
/// and the (still-stubbed) Send-to-Kobo row.
#[component]
pub(super) fn BdExportMenu(uuid: String, has_ebook: bool, has_audio: bool) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "bd-export",
            button {
                class: "btn lg ghost bd-export-trigger",
                "data-testid": "hero-export",
                r#type: "button",
                "aria-haspopup": "dialog",
                "aria-expanded": "{open()}",
                onclick: move |_| {
                    let next = !open();
                    open.set(next);
                },
                "Export"
                span { class: "bd-export-caret", aria_hidden: "true", "\u{25be}" }
            }
            if open() {
                div {
                    class: "bd-export-scrim",
                    "data-testid": "hero-export-scrim",
                    onclick: move |_| open.set(false),
                }
                BdExportPanel { uuid, has_ebook, has_audio, open }
            }
        }
    }
}

/// The open dropdown body. Split out so `onmounted` can focus it (so ESC
/// reaches the panel-level `onkeydown`), mirroring the user-menu panel.
///
/// Uses `role="dialog"` (not `role="menu"`) to match the user-menu dropdown:
/// this is a scrim-dismissed popover with plain links/buttons, not an ARIA
/// menu with roving-tabindex / arrow-key navigation.
#[component]
fn BdExportPanel(uuid: String, has_ebook: bool, has_audio: bool, open: Signal<bool>) -> Element {
    let mut open = open;
    let on_keydown = move |evt: Event<KeyboardData>| {
        if evt.key() == Key::Escape {
            evt.prevent_default();
            open.set(false);
        }
    };
    rsx! {
        div {
            class: "bd-export-panel card",
            role: "dialog",
            "aria-label": "Export options",
            "data-testid": "hero-export-panel",
            tabindex: "-1",
            onkeydown: on_keydown,
            onmounted: move |evt: MountedEvent| focus_export_panel(&evt),

            if has_ebook {
                // Plain anchor (not a router Link) so the browser performs a
                // real download; the empty `download` attr defers the filename
                // to the server's Content-Disposition.
                a {
                    class: "bd-export-item",
                    "data-testid": "export-download-epub",
                    href: "/api/ebooks/{uuid}/download",
                    download: "",
                    onclick: move |_| open.set(false),
                    span { class: "bd-export-item-label", "Download EPUB" }
                }
            }
            if has_audio {
                a {
                    class: "bd-export-item",
                    "data-testid": "export-download-audio",
                    href: "/api/audiobooks/{uuid}/download",
                    download: "",
                    onclick: move |_| open.set(false),
                    span { class: "bd-export-item-label", "Download audiobook" }
                }
            }
            // Send-to-Kindle only applies to books with an EPUB (the backend
            // errors with `NoEpub` otherwise). Reuses the interactive button,
            // styled as a menu row; the menu stays open while it reports
            // "Sending…" and raises its own toast.
            if has_ebook {
                SendToKindleButton {
                    uuid: uuid.clone(),
                    file_id: None,
                    class: "bd-export-item".to_string(),
                    testid: "hero-send-kindle".to_string(),
                }
            }
            button {
                class: "bd-export-item",
                disabled: true,
                title: "Send-to-Kobo coming soon",
                span { class: "bd-export-item-label", "Send to Kobo" }
            }
        }
    }
}

/// Focus the panel after paint so its `onkeydown` receives ESC — same
/// requestAnimationFrame timing as the user-menu panel. Web-only; SSR
/// never paints the panel, so the gate lives at the fn definition to keep
/// the `onmounted` handler body identical across targets (rule 07).
#[cfg(feature = "web")]
fn focus_export_panel(evt: &MountedEvent) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::prelude::*;

    let Some(element) = evt.try_as_web_event() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::once_into_js(move || {
        if let Some(html_el) = element.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

/// Non-web stub (SSR): nothing to focus before hydration.
#[cfg(not(feature = "web"))]
fn focus_export_panel(_evt: &MountedEvent) {}
