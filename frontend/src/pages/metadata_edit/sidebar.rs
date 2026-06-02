//! Sticky right-column sidebar for the metadata edit page: cover preview,
//! identifiers (when present), and the override-active card. Revert
//! clicks bubble to the parent so the async `delete_overrides` call and
//! navigation stay in `MetadataEditForm`.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;

/// Cover preview + identifiers + override-status sidebar.
#[component]
pub(super) fn Sidebar(
    book: EbookMetadata,
    saving: Signal<bool>,
    on_revert: EventHandler<()>,
) -> Element {
    let has_cover = book.cover_url.is_some();
    let identifiers = &book.identifiers;
    let has_override = book.has_override;

    rsx! {
        aside { class: "me-sidebar",

            // Cover preview
            div { class: "card me-sidebar-card",
                div { class: "me-sidebar-head",
                    div { class: "label", "Cover" }
                }
                div { class: "me-cover-preview",
                    Cover { book: book.clone() }
                }
                // Cover upload deferred to v2 (cover picker gallery)
                div { class: "mono me-cover-hint",
                    if has_cover {
                        "extracted from file"
                    } else {
                        "no cover available"
                    }
                }
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
                                    {ident.scheme.clone().unwrap_or_else(|| "ID".into())}
                                }
                                span { class: "mono me-ident-val",
                                    {ident.value.clone()}
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
