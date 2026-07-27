//! Shared confirm-modal shell: the backdrop-dismiss / click-through-safe
//! panel wrapper duplicated across the merge, delete, author-photo, and
//! physical-copy dialogs, plus a title/body/action-row body for the common
//! "confirm one thing" case.

use dioxus::prelude::*;

/// Visual weight of one [`ConfirmModalAction`] button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConfirmModalTone {
    Ghost,
    Danger,
}

/// One button in a [`confirm_modal_body`] action row.
#[derive(Clone, PartialEq)]
pub struct ConfirmModalAction {
    pub testid: String,
    pub label: String,
    pub tone: ConfirmModalTone,
    pub disabled: bool,
    pub on_click: EventHandler<()>,
}

/// Modal backdrop + panel shell shared by every dialog in the app: a
/// backdrop click dismisses (unless `busy`), a click inside the panel
/// itself does not bubble to the backdrop. Callers supply their own body
/// markup as `children`.
#[component]
pub fn ConfirmModal(
    testid: String,
    aria_label: String,
    dialog_class: String,
    busy: bool,
    on_dismiss: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "author-photo-modal-backdrop",
            role: "dialog",
            aria_modal: "true",
            aria_label: "{aria_label}",
            "data-testid": "{testid}",
            onclick: move |evt| {
                evt.stop_propagation();
                if !busy {
                    on_dismiss.call(());
                }
            },
            div { class: "{dialog_class}", onclick: move |evt| evt.stop_propagation(), {children} }
        }
    }
}

/// Title + body copy + action-button row — the "confirm one thing" body
/// shape shared by the physical-copy delete modals.
pub fn confirm_modal_body(title: &str, body: &str, actions: Vec<ConfirmModalAction>) -> Element {
    rsx! {
        h3 { class: "del-title", "{title}" }
        p { class: "del-copy", "{body}" }
        div { class: "del-actions",
            for action in actions {
                button {
                    key: "{action.testid}",
                    class: if action.tone == ConfirmModalTone::Danger { "del-btn-danger" } else { "del-btn-ghost" },
                    "data-testid": "{action.testid}",
                    disabled: action.disabled,
                    onclick: move |_| action.on_click.call(()),
                    "{action.label}"
                }
            }
        }
    }
}

// Every test here renders SSR markup, so the module is `server`-gated —
// under `web` its contents would be dead code and CI lints with `-D warnings`.
#[cfg(all(test, feature = "server"))]
mod tests;
