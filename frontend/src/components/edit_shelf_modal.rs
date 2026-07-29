//! "Edit shelf" modal — the full field set from shelf creation (name,
//! visibility, the Kobo sync opt-in, and the smart-shelf rule builder) in one
//! dialog. Reuses [`crate::components::shelf_rule_builder::RuleBuilder`] so
//! create and edit never duplicate the condition-row markup. Platform-agnostic
//! like [`crate::components::CreateShelfModal`]; mounted by the landing-page
//! pencil, the web `ShelfHeader` pencil, and the mobile shelf-detail menu.

use dioxus::prelude::*;
use omnibus_shared::{MatchMode, Shelf, ShelfKind, ShelfRule, UpdateShelfRequest};

use crate::components::create_shelf_modal::VisibilityToggle;
use crate::components::shelf_rule_builder::{RuleBuilder, RuleDraft};
use crate::{data, use_server_url};

// Every test here renders SSR markup, so the module is `server`-gated —
// under `web` its contents would be dead code and CI lints with `-D warnings`.
#[cfg(all(test, feature = "server"))]
mod tests;

/// Prefills from the shelf's current name / visibility / Kobo opt-in / rules
/// and saves the whole set via [`data::update_shelf`] in one request. Kind is
/// shown but not editable — [`UpdateShelfRequest`] has no `kind` field. Emits
/// the updated shelf via `on_saved` so the caller can bump its reload signal
/// and refetch.
#[component]
pub fn EditShelfModal(
    shelf: Shelf,
    on_close: EventHandler<()>,
    on_saved: EventHandler<Shelf>,
) -> Element {
    let server_url = use_server_url();
    let id = shelf.id;
    let is_smart = shelf.kind == ShelfKind::Smart;
    let kind_label = match shelf.kind {
        ShelfKind::Smart => "Smart shelf",
        ShelfKind::Manual => "Hand-picked shelf",
        ShelfKind::Wishlist => "Wishlist",
    };
    // A system shelf (Wishlist) rejects the Kobo opt-in server-side
    // (`ShelfError::SystemShelf`) — hide the toggle so the UI never offers it,
    // mirroring the shelf-detail ⋯-menu gating.
    let show_kobo = !shelf.kind.is_system();
    let mut name = use_signal(|| shelf.name.clone());
    let mut visibility = use_signal(|| shelf.visibility);
    let mut sync_to_kobo = use_signal(|| shelf.sync_to_kobo);
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
        let trimmed = name().trim().to_string();
        if trimmed.is_empty() {
            error.set(Some("Name is required.".into()));
            return;
        }
        let mut req = UpdateShelfRequest {
            name: Some(trimmed),
            visibility: Some(visibility()),
            ..Default::default()
        };
        if show_kobo {
            req.sync_to_kobo = Some(sync_to_kobo());
        }
        if is_smart {
            let wire: Vec<ShelfRule> = rules.read().iter().filter_map(RuleDraft::to_rule).collect();
            if wire.is_empty() {
                error.set(Some("Add at least one condition.".into()));
                return;
            }
            req.match_mode = Some(match_mode());
            req.rules = Some(wire);
        }
        let url = submit_url.clone();
        let on_saved = on_saved;
        saving.set(true);
        error.set(None);
        spawn(async move {
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
            "data-testid": "edit-shelf-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),

                div { class: "shelf-modal-head",
                    input {
                        r#type: "text",
                        class: "shelf-name-input",
                        placeholder: "Shelf name\u{2026}",
                        "data-testid": "edit-shelf-name",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                    span { class: "shelf-badge", "{kind_label}" }
                    VisibilityToggle { visibility: visibility(), on_change: move |v| visibility.set(v) }
                }

                if is_smart {
                    div { class: "shelf-modal-body",
                        RuleBuilder {
                            match_mode: match_mode(),
                            rules,
                            on_match_mode: move |m| match_mode.set(m),
                            server_url: server_url.clone(),
                        }
                    }
                }

                if show_kobo {
                    div { class: "shelf-modal-kobo",
                        span { class: "shelf-kobo-label", "Sync to Kobo" }
                        div { class: "shelf-kobo-toggle",
                            button {
                                r#type: "button",
                                class: "shelf-toggle-btn",
                                "aria-pressed": if !sync_to_kobo() { "true" } else { "false" },
                                "data-testid": "edit-shelf-kobo-off",
                                onclick: move |_| sync_to_kobo.set(false),
                                "Off"
                            }
                            button {
                                r#type: "button",
                                class: "shelf-toggle-btn",
                                "aria-pressed": if sync_to_kobo() { "true" } else { "false" },
                                "data-testid": "edit-shelf-kobo-on",
                                onclick: move |_| sync_to_kobo.set(true),
                                "On"
                            }
                        }
                    }
                }

                if let Some(msg) = error() {
                    p {
                        role: "alert",
                        class: "shelf-modal-error",
                        "data-testid": "edit-shelf-error",
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
                        "data-testid": "edit-shelf-save",
                        disabled: saving(),
                        onclick: on_save,
                        if saving() { "Saving\u{2026}" } else { "Save" }
                    }
                }
            }
        }
    }
}
