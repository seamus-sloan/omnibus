//! Sticky right-column sidebar for the metadata edit page: cover
//! upload/revert (delegated to `cover_editor::CoverEditor`), identifiers,
//! the OPF-export card, and the override-active card. Full-override revert
//! bubbles to the parent `MetadataEditForm`; cover-only revert stays local,
//! updating `live_book` so the "Override active" card tracks it.

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, OpfExportResult};

use super::cover_editor::CoverEditor;
use crate::{data, use_server_url};

/// Cover preview + identifiers + override-status sidebar.
#[component]
pub(super) fn Sidebar(
    book: EbookMetadata,
    saving: Signal<bool>,
    on_revert: EventHandler<()>,
) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let export_uuid = uuid.clone();
    let identifiers = book.identifiers.clone();
    // Tracks the merged book returned by a cover upload/revert so the
    // "Override active" card reflects a cover-only change immediately —
    // `book` itself is the static snapshot the parent loaded once.
    let mut live_book = use_signal(|| book.clone());
    let has_override = live_book().has_override;

    rsx! {
        aside { class: "me-sidebar",

            CoverEditor {
                book: book.clone(),
                uuid,
                on_change: move |updated| live_book.set(updated),
            }

            // Identifiers (read-only for v1)
            if !identifiers.is_empty() {
                div { class: "card me-sidebar-card",
                    div { class: "label", style: "margin-bottom: 12px;", "Identifiers" }
                    div { class: "me-ident-list",
                        for ident in identifiers.iter() {
                            div {
                                key: "{ident.scheme:?}-{ident.value}",
                                class: "me-ident-row",
                                span { class: "label me-ident-key",
                                    {ident.scheme.as_deref().unwrap_or("ID")}
                                }
                                span { class: "mono me-ident-val",
                                    {ident.value.as_str()}
                                }
                            }
                        }
                    }
                }
            }

            ExportCard { uuid: export_uuid, saving }

            // Override status
            if has_override {
                div { class: "card me-sidebar-card",
                    div { class: "label", style: "margin-bottom: 8px;", "Override active" }
                    p { class: "mono", style: "font-size: 11px; color: var(--ink-2);",
                        "This book has metadata overrides. Saving will update them; discarding will leave existing overrides intact."
                    }
                    button {
                        class: "btn ghost sm",
                        style: "margin-top: 10px; width: 100%; justify-content: center;",
                        "data-testid": "revert-overrides",
                        disabled: saving(),
                        onclick: move |_| on_revert.call(()),
                        "Revert to scanned values"
                    }
                }
            }
        }
    }
}

/// "Export to OPF" card: writes the book's merged metadata to its
/// `metadata.opf` sidecar and reports the resulting path / backup status.
/// Not dirty-dependent, so it owns its own state instead of touching the
/// parent form's save flow.
#[component]
fn ExportCard(uuid: String, saving: Signal<bool>) -> Element {
    let server_url = use_server_url();
    let mut exporting = use_signal(|| false);
    let mut status: Signal<Option<Result<OpfExportResult, String>>> = use_signal(|| None);

    let on_export = move |_| {
        let uuid = uuid.clone();
        let server_url = server_url.clone();
        exporting.set(true);
        status.set(None);
        spawn(async move {
            let result = data::export_opf(&server_url, &uuid)
                .await
                .map_err(|e| format!("Export failed: {e}"));
            status.set(Some(result));
            exporting.set(false);
        });
    };

    rsx! {
        div { class: "card me-sidebar-card",
            div { class: "label", style: "margin-bottom: 8px;", "Export" }
            p { class: "mono", style: "font-size: 11px; color: var(--ink-2);",
                "Write a metadata.opf sidecar next to the book's EPUB with the current (scanned + override) metadata."
            }
            button {
                class: "btn ghost sm",
                style: "margin-top: 10px; width: 100%; justify-content: center;",
                "data-testid": "me-export-opf",
                disabled: exporting() || saving(),
                onclick: on_export,
                if exporting() { "Exporting\u{2026}" } else { "Export to OPF" }
            }
            match status() {
                Some(Ok(result)) => rsx! {
                    p {
                        class: "mono",
                        "data-testid": "me-export-status",
                        style: "font-size: 11px; color: var(--ink-2); margin-top: 8px;",
                        "Wrote {result.path}"
                        if result.backed_up {
                            " (previous file backed up to metadata.opf.bak)"
                        }
                    }
                },
                Some(Err(msg)) => rsx! {
                    p {
                        class: "mono",
                        "data-testid": "me-export-status",
                        style: "font-size: 11px; color: var(--bad); margin-top: 8px;",
                        "{msg}"
                    }
                },
                None => rsx! {},
            }
        }
    }
}
