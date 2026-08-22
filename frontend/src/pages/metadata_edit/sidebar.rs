//! Sticky right-column sidebar for the metadata edit page: cover
//! upload/revert (delegated to `cover_editor::CoverEditor`), identifiers,
//! and the override-active card. Full-override revert bubbles to the parent
//! `MetadataEditForm`; cover-only revert stays local, updating `live_book` so
//! the "Override active" card tracks it.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use super::cover_editor::CoverEditor;

/// Cover preview + identifiers + override-status sidebar.
#[component]
pub(super) fn Sidebar(
    book: EbookMetadata,
    saving: Signal<bool>,
    on_revert: EventHandler<()>,
) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let identifiers = book.identifiers.clone();
    // Tracks the merged book returned by a cover write, so the "Override
    // active" card reflects a cover-only change immediately.
    //
    // Seeded from `book` and re-seeded when it changes: the write can come
    // from the sidebar's own `CoverEditor` (through `on_change` below) *or*
    // from the compare view's cover row on the other side of the page, which
    // reaches here as a new `book` prop. A seed-once signal would show the
    // second case a stale card until the next page load.
    let mut live_book = use_signal(|| book.clone());
    use_effect(use_reactive!(|book| live_book.set(book.clone())));
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
