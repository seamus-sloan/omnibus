//! "Edit shelf" modal — name, visibility, the Kobo sync opt-in, and the
//! smart-shelf rule builder in one dialog. Reuses
//! [`crate::components::shelf_rule_builder::RuleBuilder`] like
//! [`crate::components::CreateShelfModal`]; mounted by the landing-page
//! pencil, the web `ShelfHeader` pencil, and the mobile shelf-detail menu.

use dioxus::prelude::*;
use omnibus_shared::{MatchMode, Shelf, ShelfKind, ShelfRule, UpdateShelfRequest, Visibility};

use crate::components::create_shelf_modal::VisibilityToggle;
use crate::components::shelf_rule_builder::{RuleBuilder, RuleDraft};
use crate::{data, use_server_url};

// SSR render tests — `server`-gated so `web` builds don't carry dead code.
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
    let kind_label = kind_label(shelf.kind);
    // System shelves reject the Kobo opt-in (`ShelfError::SystemShelf`) — never offer it.
    let show_kobo = !shelf.kind.is_system();
    let name = use_signal(|| shelf.name.clone());
    let visibility = use_signal(|| shelf.visibility);
    let sync_to_kobo = use_signal(|| shelf.sync_to_kobo);
    let mut match_mode = use_signal(|| shelf.match_mode.unwrap_or(MatchMode::All));
    let rules = use_signal(|| initial_rule_drafts(&shelf.rules));
    let error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);

    let on_save = build_on_save(
        server_url.clone(),
        id,
        show_kobo,
        is_smart,
        EditShelfFormSignals {
            name,
            visibility,
            sync_to_kobo,
            match_mode,
            rules,
            error,
            saving,
        },
        on_saved,
    );

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "edit-shelf-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),

                {edit_shelf_head(name, kind_label, visibility)}

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
                    {edit_shelf_kobo_toggle(sync_to_kobo)}
                }

                if let Some(msg) = error() {
                    p {
                        role: "alert",
                        class: "shelf-modal-error",
                        "data-testid": "edit-shelf-error",
                        "{msg}"
                    }
                }

                {edit_shelf_foot(saving(), on_close, on_save)}
            }
        }
    }
}

/// Read-only kind badge text for a shelf's kind.
fn kind_label(kind: ShelfKind) -> &'static str {
    match kind {
        ShelfKind::Smart => "Smart shelf",
        ShelfKind::Manual => "Hand-picked shelf",
        ShelfKind::Wishlist => "Wishlist",
    }
}

/// Editable rule drafts prefilled from a shelf's saved rules, or one blank
/// draft for a smart shelf that has none yet.
fn initial_rule_drafts(rules: &[ShelfRule]) -> Vec<RuleDraft> {
    let drafts: Vec<RuleDraft> = rules.iter().map(RuleDraft::from_rule).collect();
    if drafts.is_empty() {
        vec![RuleDraft::default()]
    } else {
        drafts
    }
}

/// Form-state signals [`build_on_save`] reads/writes. `Copy` (Dioxus
/// signals), so grouping them keeps the function under clippy's
/// too-many-arguments cap without changing call-site ergonomics.
#[derive(Clone, Copy)]
struct EditShelfFormSignals {
    name: Signal<String>,
    visibility: Signal<Visibility>,
    sync_to_kobo: Signal<bool>,
    match_mode: Signal<MatchMode>,
    rules: Signal<Vec<RuleDraft>>,
    error: Signal<Option<String>>,
    saving: Signal<bool>,
}

/// Builds the save handler: validates via [`build_update_request`], then
/// saves the whole form in one request and reports the result back through
/// `error`/`saving`/`on_saved`.
fn build_on_save(
    server_url: String,
    id: i64,
    show_kobo: bool,
    is_smart: bool,
    sig: EditShelfFormSignals,
    on_saved: EventHandler<Shelf>,
) -> EventHandler<MouseEvent> {
    let EditShelfFormSignals {
        name,
        visibility,
        sync_to_kobo,
        match_mode,
        rules,
        mut error,
        mut saving,
    } = sig;
    EventHandler::new(move |_| {
        if saving() {
            return;
        }
        let req = build_update_request(
            &name(),
            visibility(),
            show_kobo,
            sync_to_kobo(),
            is_smart,
            match_mode(),
            &rules.read(),
        );
        let req = match req {
            Ok(req) => req,
            Err(msg) => {
                error.set(Some(msg));
                return;
            }
        };
        let url = server_url.clone();
        saving.set(true);
        error.set(None);
        spawn(async move {
            match data::update_shelf(&url, id, req).await {
                Ok(updated) => on_saved.call(updated),
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    })
}

/// Modal head: name input, read-only kind badge, and the shared visibility
/// toggle. Split out of [`EditShelfModal`] to keep it under the line cap,
/// mirroring `create_shelf_modal`'s `create_shelf_head`.
fn edit_shelf_head(
    mut name: Signal<String>,
    kind_label: &'static str,
    mut visibility: Signal<Visibility>,
) -> Element {
    rsx! {
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
    }
}

/// The Kobo sync opt-in row (off/on segmented toggle). Only mounted for
/// non-system shelves — see `show_kobo` at the call site.
fn edit_shelf_kobo_toggle(mut sync_to_kobo: Signal<bool>) -> Element {
    rsx! {
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
}

/// Modal foot: cancel + save buttons; save disables and swaps its label
/// while a save request is in flight.
fn edit_shelf_foot(
    saving: bool,
    on_close: EventHandler<()>,
    on_save: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
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
                disabled: saving,
                onclick: move |e| on_save.call(e),
                if saving { "Saving\u{2026}" } else { "Save" }
            }
        }
    }
}

/// Validate the current form state and build the save request, or return the
/// message the modal renders inline. Two branches reject before any network
/// call: an empty (post-trim) name, and — for a smart shelf only — an empty
/// encoded rule set.
fn build_update_request(
    name: &str,
    visibility: Visibility,
    show_kobo: bool,
    sync_to_kobo: bool,
    is_smart: bool,
    match_mode: MatchMode,
    rules: &[RuleDraft],
) -> Result<UpdateShelfRequest, String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("Name is required.".into());
    }
    let mut req = UpdateShelfRequest {
        name: Some(trimmed),
        visibility: Some(visibility),
        ..Default::default()
    };
    if show_kobo {
        req.sync_to_kobo = Some(sync_to_kobo);
    }
    if is_smart {
        let wire: Vec<ShelfRule> = rules.iter().filter_map(RuleDraft::to_rule).collect();
        if wire.is_empty() {
            return Err("Add at least one condition.".into());
        }
        req.match_mode = Some(match_mode);
        req.rules = Some(wire);
    }
    Ok(req)
}
