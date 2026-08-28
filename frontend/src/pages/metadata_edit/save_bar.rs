//! Sticky bottom save bar for the metadata edit page: dirty-field
//! summary on the left, error text, then `Discard edits` link + `Save`
//! button.
//!
//! Save click invokes the parent-provided `on_save` so the async
//! `save_overrides` call and navigation stay in `MetadataEditForm`.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Dirty-tracking memos forwarded to the save bar.
///
/// `cover_replaced` is not a dirty *field*: a cover write lands on the server
/// the moment it is picked, so it can never be part of what Save sends. It is
/// tracked here anyway because the bar's job is to describe the state of the
/// editor, and "No changes" over a book whose cover just changed is a lie the
/// reader acts on (#2241).
#[derive(Clone, Copy, PartialEq)]
pub(super) struct DirtyState {
    pub(super) fields: Memo<Vec<&'static str>>,
    pub(super) count: Memo<usize>,
    pub(super) cover_replaced: Signal<bool>,
}

/// In-flight save status: in-progress flag plus last error message.
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
        cover_replaced,
    } = dirty;
    // Save is an exit, not only a write: once the cover has been replaced the
    // editor holds a change the reader can't take back here, so the button
    // has to let them leave through it rather than sitting greyed out.
    let can_leave_via_save = dirty_count() > 0 || cover_replaced();
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
            } else if cover_replaced() {
                span { class: "mono me-dirty-label", "data-testid": "me-cover-replaced",
                    "Cover replaced \u{b7} already saved"
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
                // Leaves the field edits unsent. Labelled for what it
                // actually drops: a replaced cover is already on the server
                // and no button on this page can take it back.
                Link {
                    to: Route::BookDetail { uuid: uuid.clone() },
                    class: "btn ghost",
                    "data-testid": "me-discard",
                    "Discard edits"
                }

                // Save
                button {
                    class: "btn primary",
                    "data-testid": "me-save",
                    disabled: !can_leave_via_save || saving(),
                    onclick: move |_| on_save.call(()),
                    {
                        if saving() {
                            "Saving\u{2026}".to_string()
                        } else if dirty_count() > 0 {
                            format!("Save \u{b7} {} field{}", dirty_count(), if dirty_count() != 1 { "s" } else { "" })
                        } else {
                            "Done".to_string()
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
