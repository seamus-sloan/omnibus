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

/// Forward to `on_dismiss` unless a mutation is in flight — the shared
/// backdrop-dismiss gate every modal call site relies on. Split out from the
/// backdrop's `onclick` so the branch is directly unit-testable without a
/// simulated click.
fn dismiss_unless_busy(busy: bool, on_dismiss: EventHandler<()>) {
    if !busy {
        on_dismiss.call(());
    }
}

/// Modal backdrop + panel shell shared by every dialog in the app: a
/// backdrop click dismisses (unless `busy`), a click inside the panel
/// itself does not bubble to the backdrop. Callers supply their own body
/// markup as `children`, and optionally a `head` slot (title + close
/// button) rendered above it — `backdrop_class` defaults to the original
/// fixed class so every pre-existing caller is unaffected; a caller
/// with its own backdrop chrome (centered vs. bottom-sheet, blur, z-index)
/// overrides it. The `head` slot stays a raw `Element` rather than a
/// `title: String` because callers disagree on heading tag and class
/// (`ModalShell`'s `h3`/`users-modal-*` vs. `DrillIn`'s `h4`/`st-drill-*`
/// plus a leading grabber bar) — forcing one shape would change either
/// caller's rendered markup, which is the one thing this refactor must not
/// do.
///
/// 8 heterogeneous props left ungrouped: 16 call sites make a chrome struct
/// not worth the churn.
#[component]
pub fn ConfirmModal(
    testid: String,
    aria_label: String,
    #[props(default = "author-photo-modal-backdrop".to_string())] backdrop_class: String,
    dialog_class: String,
    busy: bool,
    on_dismiss: EventHandler<()>,
    #[props(default)] head: Option<Element>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "{backdrop_class}",
            role: "dialog",
            aria_modal: "true",
            aria_label: "{aria_label}",
            "data-testid": "{testid}",
            onclick: move |evt| {
                evt.stop_propagation();
                dismiss_unless_busy(busy, on_dismiss);
            },
            div { class: "{dialog_class}", onclick: move |evt| evt.stop_propagation(),
                if let Some(h) = head { {h} }
                {children}
            }
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
