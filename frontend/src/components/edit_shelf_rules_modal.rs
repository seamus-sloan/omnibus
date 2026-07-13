//! "Edit rules" modal for an existing smart shelf (issue #987) — reuses
//! [`crate::components::shelf_rule_builder::RuleBuilder`] so create and edit
//! never duplicate the condition-row markup. Platform-agnostic like
//! [`crate::components::CreateShelfModal`]; mounted by both the web
//! `ShelfHeader` actions menu and the mobile shelf-detail actions menu.

use dioxus::prelude::*;
use omnibus_shared::{MatchMode, Shelf, ShelfRule, UpdateShelfRequest};

use crate::components::shelf_rule_builder::{RuleBuilder, RuleDraft};
use crate::{data, use_server_url};

/// Prefills from `shelf.match_mode`/`shelf.rules` and saves via
/// [`data::update_shelf`] — the same call rename/visibility already use.
/// Emits the updated shelf via `on_saved` so the caller can bump its reload
/// signal and refetch membership.
#[component]
pub fn EditShelfRulesModal(
    shelf: Shelf,
    on_close: EventHandler<()>,
    on_saved: EventHandler<Shelf>,
) -> Element {
    let server_url = use_server_url();
    let id = shelf.id;
    let mut match_mode = use_signal(|| shelf.match_mode.unwrap_or(MatchMode::All));
    let rules = use_signal(|| {
        let drafts: Vec<RuleDraft> = shelf.rules.iter().map(RuleDraft::from_rule).collect();
        if drafts.is_empty() {
            vec![RuleDraft::default()]
        } else {
            drafts
        }
    });
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    let submit_url = server_url.clone();
    let on_save = move |_| {
        if saving() {
            return;
        }
        let wire: Vec<ShelfRule> = rules.read().iter().filter_map(RuleDraft::to_rule).collect();
        if wire.is_empty() {
            error.set(Some("Add at least one condition.".into()));
            return;
        }
        let url = submit_url.clone();
        let mode = match_mode();
        let on_saved = on_saved;
        saving.set(true);
        error.set(None);
        spawn(async move {
            let req = UpdateShelfRequest {
                rules: Some(wire),
                match_mode: Some(mode),
                ..Default::default()
            };
            match data::update_shelf(&url, id, req).await {
                Ok(updated) => on_saved.call(updated),
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "edit-shelf-rules-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),

                div { class: "shelf-modal-head",
                    h2 { class: "shelf-modal-title", "Edit rules" }
                }

                div { class: "shelf-modal-body",
                    RuleBuilder {
                        match_mode: match_mode(),
                        rules,
                        on_match_mode: move |m| match_mode.set(m),
                        server_url: server_url.clone(),
                    }
                }

                if let Some(msg) = error() {
                    p {
                        role: "alert",
                        class: "shelf-modal-error",
                        "data-testid": "edit-rules-error",
                        "{msg}"
                    }
                }

                div { class: "shelf-modal-foot",
                    button {
                        r#type: "button",
                        class: "btn shelf-btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "btn shelf-btn-primary",
                        "data-testid": "edit-rules-save",
                        disabled: saving(),
                        onclick: on_save,
                        if saving() { "Saving\u{2026}" } else { "Save" }
                    }
                }
            }
        }
    }
}
