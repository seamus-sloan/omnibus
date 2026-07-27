//! SSR render-smoke coverage for the shared modal shell and its
//! title/body/action-row body, plus direct coverage of the backdrop's
//! busy-gate logic. Needs the `server` feature (`dioxus::ssr`).

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::test_support::render;

#[component]
fn Harness() -> Element {
    rsx! {
        ConfirmModal {
            testid: "sample-modal".to_string(),
            aria_label: "Sample modal".to_string(),
            dialog_class: "mg-modal del-modal".to_string(),
            busy: false,
            on_dismiss: move |_| {},
            {confirm_modal_body(
                "Remove this copy?",
                "This removes the physical copy from your collection.",
                vec![
                    ConfirmModalAction {
                        testid: "sample-cancel".to_string(),
                        label: "Cancel".to_string(),
                        tone: ConfirmModalTone::Ghost,
                        disabled: false,
                        on_click: EventHandler::new(|_| {}),
                    },
                    ConfirmModalAction {
                        testid: "sample-confirm".to_string(),
                        label: "I sold it".to_string(),
                        tone: ConfirmModalTone::Danger,
                        disabled: false,
                        on_click: EventHandler::new(|_| {}),
                    },
                ],
            )}
        }
    }
}

#[test]
fn confirm_modal_renders_the_backdrop_panel_title_body_and_actions() {
    let html = render(rsx! { Harness {} });
    assert!(html.contains("data-testid=\"sample-modal\""));
    assert!(html.contains("author-photo-modal-backdrop"));
    assert!(html.contains("mg-modal del-modal"));
    assert!(html.contains("Remove this copy?"));
    assert!(html.contains("This removes the physical copy from your collection."));
    assert!(html.contains("data-testid=\"sample-cancel\""));
    assert!(html.contains("del-btn-ghost"));
    assert!(html.contains("data-testid=\"sample-confirm\""));
    assert!(html.contains("del-btn-danger"));
}

#[component]
fn BusyHarness() -> Element {
    confirm_modal_body(
        "Busy",
        "In progress",
        vec![ConfirmModalAction {
            testid: "sample-busy".to_string(),
            label: "Working\u{2026}".to_string(),
            tone: ConfirmModalTone::Danger,
            disabled: true,
            on_click: EventHandler::new(|_| {}),
        }],
    )
}

#[test]
fn confirm_modal_body_disables_every_action_when_told_to() {
    let html = render(rsx! { BusyHarness {} });
    assert!(html.contains("disabled"));
}

/// Runs [`dismiss_unless_busy`] inside a live scope (needed for
/// `EventHandler::new`) and records whether it fired.
#[component]
fn DismissHarness(busy: bool, fired: Rc<RefCell<bool>>) -> Element {
    let handler = EventHandler::new(move |_| *fired.borrow_mut() = true);
    dismiss_unless_busy(busy, handler);
    rsx! {}
}

#[test]
fn dismiss_unless_busy_calls_on_dismiss_when_not_busy() {
    let fired = Rc::new(RefCell::new(false));
    render(rsx! { DismissHarness { busy: false, fired: fired.clone() } });
    assert!(*fired.borrow());
}

#[test]
fn dismiss_unless_busy_is_a_noop_while_busy() {
    let fired = Rc::new(RefCell::new(false));
    render(rsx! { DismissHarness { busy: true, fired: fired.clone() } });
    assert!(!*fired.borrow());
}
