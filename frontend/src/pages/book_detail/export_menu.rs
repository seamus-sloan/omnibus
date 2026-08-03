//! Hero "Export" dropdown — condenses the per-device send/download actions
//! (Download EPUB, Download audiobook, Send to Kindle, Send to Kobo) behind
//! one trigger so the hero CTA row stays a single primary action plus this
//! menu. Web/server only; the book-detail hero isn't compiled on mobile.

use dioxus::prelude::*;
use omnibus_shared::{kindle_email_oversize, KINDLE_WEB_UPLOAD_URL};

use crate::components::{SendToKindleButton, SendToKoboButton};
use crate::focus_after_paint::focus_after_paint;

/// The "Export" trigger + dropdown panel. Renders the download links for
/// whichever formats the book has, the interactive Send-to-Kindle button,
/// and the Send-to-Kobo KEPUB download.
#[component]
pub(super) fn BdExportMenu(ctx: BdExportContext) -> Element {
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
                BdExportPanel { ctx, open }
            }
        }
    }
}

/// The uuid, available formats, author/title (for Send-to-Kindle/Kobo
/// filenames), and EPUB size gating the oversize-email fallback. Grouped so
/// [`BdExportMenu`] and [`BdExportPanel`] both stay under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct BdExportContext {
    pub uuid: String,
    pub has_ebook: bool,
    pub has_audio: bool,
    pub book_author: String,
    pub book_title: String,
    pub epub_size_bytes: Option<i64>,
}

/// The open dropdown body. Split out so `onmounted` can focus it (so ESC
/// reaches the panel-level `onkeydown`), mirroring the user-menu panel.
///
/// Uses `role="dialog"` (not `role="menu"`) to match the user-menu dropdown:
/// this is a scrim-dismissed popover with plain links/buttons, not an ARIA
/// menu with roving-tabindex / arrow-key navigation.
#[component]
fn BdExportPanel(ctx: BdExportContext, open: Signal<bool>) -> Element {
    let BdExportContext {
        uuid,
        has_ebook,
        has_audio,
        book_author,
        book_title,
        epub_size_bytes,
    } = ctx;
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
            onmounted: move |evt: MountedEvent| focus_after_paint(&evt),

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
            // "Sending…" and raises its own toast. When the EPUB exceeds
            // Kindle's email cap, the button can't work — swap in a disabled
            // row that explains why and links to the web uploader instead.
            if has_ebook {
                // `u64::try_from` over an `as` cast so a negative size (corrupt
                // DB row / sentinel) can't wrap into a huge value and wrongly
                // hide the email button.
                if epub_size_bytes
                    .and_then(|n| u64::try_from(n).ok())
                    .is_some_and(kindle_email_oversize)
                {
                    KindleOversizeItem { open }
                } else {
                    SendToKindleButton {
                        uuid: uuid.clone(),
                        file_id: None,
                        class: "bd-export-item".to_string(),
                        testid: "hero-send-kindle".to_string(),
                    }
                }
            }
            // Send-to-Kobo writes the KEPUB straight onto a plugged-in Kobo
            // (Chrome/Edge), or downloads it to copy over. Only applies to books
            // with an ebook, since the endpoint converts the EPUB. Reuses the
            // interactive button (same menu-stays-open + own-toast model as
            // Send-to-Kindle above).
            if has_ebook {
                SendToKoboButton {
                    uuid: uuid.clone(),
                    book_author: book_author.clone(),
                    book_title: book_title.clone(),
                    class: "bd-export-item".to_string(),
                    testid: "hero-send-kobo".to_string(),
                }
            }
        }
    }
}

/// Disabled Send-to-Kindle row for an EPUB over Kindle's 50 MB email cap. The
/// greyed row reads as unavailable; the visible sub-note below it links to
/// Amazon's Send to Kindle page (up to 200 MB), which the email path can't
/// match. Rendered as inert markup (no button/handler) since there is nothing
/// to send — the sub-note carries the reason and the way forward.
#[component]
fn KindleOversizeItem(open: Signal<bool>) -> Element {
    let mut open = open;
    rsx! {
        div {
            class: "bd-export-item bd-export-item-muted",
            "data-testid": "hero-send-kindle-oversize",
            aria_disabled: "true",
            span { class: "bd-export-item-label", "Send to Kindle" }
        }
        a {
            class: "bd-export-subnote",
            "data-testid": "hero-send-kindle-web-link",
            href: KINDLE_WEB_UPLOAD_URL,
            target: "_blank",
            rel: "noopener noreferrer",
            onclick: move |_| open.set(false),
            "Too large to email. Upload it on Amazon's Send to Kindle page (up to 200 MB) \u{2192}"
        }
    }
}
