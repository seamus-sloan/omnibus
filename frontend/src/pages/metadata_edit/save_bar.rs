//! Sticky bottom save bar for the metadata edit page: dirty-field
//! summary on the left, error text, then `Discard` link + `Save` button.
//!
//! Save click invokes the parent-provided `on_save` so the async
//! `save_overrides` call and navigation stay in `MetadataEditForm`.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Dirty-tracking memos forwarded to the save bar. Grouped because
/// the list of edited field labels and their count are always derived
/// from the same parent computation.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct DirtyState {
    pub(super) fields: Memo<Vec<&'static str>>,
    pub(super) count: Memo<usize>,
}

/// In-flight save status: whether a POST is on the wire, and the most
/// recent error message if the last save failed. Grouped because the
/// button label and the inline error text are both functions of the
/// same lifecycle.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SaveStatus {
    pub(super) saving: Signal<bool>,
    pub(super) error: Signal<Option<String>>,
}

/// Dirty-field summary + Discard/Save actions.
#[component]
pub(super) fn SaveBar(
    uuid: String,
    dirty: DirtyState,
    status: SaveStatus,
    on_save: EventHandler<()>,
) -> Element {
    let DirtyState {
        fields: dirty_fields,
        count: dirty_count,
    } = dirty;
    let SaveStatus {
        saving,
        error: save_error,
    } = status;
    rsx! {
        div { class: "me-save-bar",
            if dirty_count() > 0 {
                span { class: "me-dirty-dot" }
                span { class: "me-dirty-label",
                    {format!("{} field{} edited", dirty_count(), if dirty_count() != 1 { "s" } else { "" })}
                }
                span { class: "mono me-dirty-names",
                    {dirty_fields().join(" \u{b7} ")}
                }
            } else {
                span { class: "mono me-dirty-label", style: "color: var(--ink-3);",
                    "No changes"
                }
            }

            if let Some(err) = save_error() {
                span { class: "mono", style: "color: var(--bad); font-size: 12px; margin-left: 8px;",
                    "{err}"
                }
            }

            div { class: "me-save-actions",
                // Discard — navigates back without saving
                Link {
                    to: Route::BookDetail { uuid: uuid.clone() },
                    class: "btn ghost",
                    "data-testid": "me-discard",
                    "Discard"
                }

                // Save
                button {
                    class: "btn primary",
                    "data-testid": "me-save",
                    disabled: dirty_count() == 0 || saving(),
                    onclick: move |_| on_save.call(()),
                    {
                        if saving() {
                            "Saving\u{2026}".to_string()
                        } else if dirty_count() > 0 {
                            format!("Save \u{b7} {} field{}", dirty_count(), if dirty_count() != 1 { "s" } else { "" })
                        } else {
                            "Save".to_string()
                        }
                    }
                }
            }
        }
    }
}
