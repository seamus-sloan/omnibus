//! Shelf detail header: back link, kind/visibility/Kobo/owner badges, the
//! name (with inline rename), and the owner-only actions menu (edit rules,
//! rename, toggle visibility, toggle Kobo sync, delete).

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{Shelf, ShelfKind, UpdateShelfRequest, Visibility};

use crate::{data, use_server_url, Route};

/// Header: back link, badges, name, and the actions menu. `on_changed` fires
/// after a successful rename / visibility change so the parent refetches and
/// the header reflects the new value; `on_edit_rules` opens the smart-shelf
/// rule editor.
#[component]
pub(super) fn ShelfHeader(
    shelf: Shelf,
    on_edit_rules: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let renaming = use_signal(|| false);
    let mut draft_name = use_signal(|| shelf.name.clone());
    let menu_open = use_signal(|| false);

    // Owner/admin gating; `None` until the boot effect resolves the viewer, so
    // controls stay hidden on SSR + first paint (hydration parity, rule 07). The
    // server enforces the same rule (`shelf_for_edit`).
    let viewer = crate::use_current_user_summary()();
    let owner_id = shelf.owner_user_id;
    // A system shelf (Wishlist) is locked even for its owner/admin: no rename,
    // delete, visibility toggle, or rule edit. Mirrors the server-side
    // `ShelfError::SystemShelf` rejection so the UI never offers a 403 action.
    let can_manage = !shelf.kind.is_system()
        && viewer
            .as_ref()
            .is_some_and(|u| u.id == owner_id || u.is_admin);
    // Suppressed on a wishlist: its name already opens with the owner's username.
    let show_attribution =
        viewer.as_ref().is_some_and(|u| u.id != owner_id) && shelf.kind != ShelfKind::Wishlist;

    let id = shelf.id;
    let is_smart = shelf.kind == ShelfKind::Smart;
    let kind_label = match shelf.kind {
        ShelfKind::Smart => "Smart",
        ShelfKind::Manual => "Hand-picked",
        ShelfKind::Wishlist => "Wishlist",
    };
    let vis_label = match shelf.visibility {
        Visibility::Private => "Private",
        Visibility::Public => "Public",
    };
    let next_vis = match shelf.visibility {
        Visibility::Private => Visibility::Public,
        Visibility::Public => Visibility::Private,
    };

    let syncs_to_kobo = shelf.sync_to_kobo;

    let on_delete = build_on_delete(server_url.clone(), id, nav);
    let on_toggle_vis =
        build_on_toggle_vis(server_url.clone(), id, next_vis, menu_open, on_changed);
    let on_toggle_kobo = build_on_toggle_kobo(
        server_url.clone(),
        id,
        !syncs_to_kobo,
        menu_open,
        on_changed,
    );
    let on_rename_save =
        build_on_rename_save(server_url.clone(), id, renaming, draft_name, on_changed);

    rsx! {
        header { class: "shelf-detail-header", "data-testid": "shelf-detail-header",
            Link { to: Route::Landing {}, class: "shelf-back", "\u{2190} All books" }
            div { class: "shelf-badges",
                span { class: "shelf-badge", "{kind_label}" }
                span { class: "shelf-badge shelf-badge--vis", "{vis_label}" }
                if syncs_to_kobo {
                    span {
                        class: "shelf-badge shelf-badge--kobo",
                        "data-testid": "shelf-kobo-badge",
                        "Syncs to Kobo"
                    }
                }
                if show_attribution {
                    span {
                        class: "shelf-badge shelf-badge--owner",
                        "data-testid": "shelf-owner-attribution",
                        "by {shelf.owner_username}"
                    }
                }
            }
            div { class: "shelf-title-row",
                if renaming() {
                    input {
                        r#type: "text",
                        class: "shelf-name-input",
                        "data-testid": "shelf-rename-input",
                        value: "{draft_name}",
                        oninput: move |e| draft_name.set(e.value()),
                    }
                    button {
                        r#type: "button", class: "btn shelf-btn-primary",
                        "data-testid": "shelf-rename-save",
                        onclick: move |_| on_rename_save.call(()),
                        "Save"
                    }
                } else {
                    h1 { class: "shelf-title", "{shelf.name}" }
                    if can_manage {
                        {shelf_actions_menu(
                            is_smart,
                            syncs_to_kobo,
                            menu_open,
                            renaming,
                            ShelfMenuActions {
                                edit_rules: on_edit_rules,
                                toggle_vis: on_toggle_vis,
                                toggle_kobo: on_toggle_kobo,
                                delete: on_delete,
                            },
                        )}
                    }
                }
            }
        }
    }
}

/// Builds the delete-shelf handler: deletes then navigates back to the
/// library on success.
fn build_on_delete(server_url: String, id: i64, nav: dioxus_router::Navigator) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        spawn(async move {
            if data::delete_shelf(&url, id).await.is_ok() {
                nav.push(Route::Landing {});
            }
        });
    })
}

/// Builds the visibility-toggle handler: closes the actions menu, flips
/// visibility, and refetches on success.
fn build_on_toggle_vis(
    server_url: String,
    id: i64,
    next_vis: Visibility,
    mut menu_open: Signal<bool>,
    on_changed: EventHandler<()>,
) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        menu_open.set(false);
        spawn(async move {
            let req = UpdateShelfRequest {
                visibility: Some(next_vis),
                ..Default::default()
            };
            if data::update_shelf(&url, id, req).await.is_ok() {
                on_changed.call(());
            }
        });
    })
}

/// Builds the Kobo sync-opt-in handler: flips `sync_to_kobo` and refetches on
/// success. Toggling immediately changes what the next device sync returns —
/// there is no separate publish step (#924 AC2).
fn build_on_toggle_kobo(
    server_url: String,
    id: i64,
    next: bool,
    mut menu_open: Signal<bool>,
    on_changed: EventHandler<()>,
) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        menu_open.set(false);
        spawn(async move {
            let req = UpdateShelfRequest {
                sync_to_kobo: Some(next),
                ..Default::default()
            };
            if data::update_shelf(&url, id, req).await.is_ok() {
                on_changed.call(());
            }
        });
    })
}

/// Builds the rename-save handler: saves the draft name and refetches on
/// success.
fn build_on_rename_save(
    server_url: String,
    id: i64,
    mut renaming: Signal<bool>,
    draft_name: Signal<String>,
    on_changed: EventHandler<()>,
) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        let name = draft_name();
        renaming.set(false);
        spawn(async move {
            let req = UpdateShelfRequest {
                name: Some(name),
                ..Default::default()
            };
            if data::update_shelf(&url, id, req).await.is_ok() {
                on_changed.call(());
            }
        });
    })
}

/// The actions-menu handlers, bundled so [`shelf_actions_menu`] stays inside
/// clippy's argument-count limit.
struct ShelfMenuActions {
    edit_rules: EventHandler<()>,
    toggle_vis: EventHandler<()>,
    toggle_kobo: EventHandler<()>,
    delete: EventHandler<()>,
}

/// Owner-only actions trigger + dropdown: edit rules (smart shelves only),
/// rename, toggle visibility, toggle Kobo sync, delete. Split out of
/// [`ShelfHeader`] to keep it under the line cap, mirroring `body::member_grid`'s
/// plain-fn split.
fn shelf_actions_menu(
    is_smart: bool,
    syncs_to_kobo: bool,
    mut menu_open: Signal<bool>,
    mut renaming: Signal<bool>,
    actions: ShelfMenuActions,
) -> Element {
    let ShelfMenuActions {
        edit_rules: on_edit_rules,
        toggle_vis: on_toggle_vis,
        toggle_kobo: on_toggle_kobo,
        delete: on_delete,
    } = actions;
    let kobo_label = if syncs_to_kobo {
        "Stop syncing to Kobo"
    } else {
        "Sync to Kobo"
    };
    rsx! {
        div { class: "shelf-actions",
            button {
                r#type: "button",
                class: "shelf-actions-btn",
                "data-testid": "shelf-actions",
                "aria-label": "Shelf actions",
                onclick: move |_| menu_open.toggle(),
                "\u{22EF}"
            }
            if menu_open() {
                div { class: "shelf-actions-menu",
                    if is_smart {
                        button {
                            r#type: "button", class: "shelf-menu-item",
                            "data-testid": "shelf-edit-rules",
                            onclick: move |_| { menu_open.set(false); on_edit_rules.call(()); },
                            "Edit rules"
                        }
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item",
                        "data-testid": "shelf-rename",
                        onclick: move |_| { menu_open.set(false); renaming.set(true); },
                        "Rename"
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item",
                        "data-testid": "shelf-toggle-visibility",
                        onclick: move |_| on_toggle_vis.call(()),
                        "Change visibility"
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item",
                        "data-testid": "shelf-toggle-kobo",
                        "aria-pressed": if syncs_to_kobo { "true" } else { "false" },
                        onclick: move |_| on_toggle_kobo.call(()),
                        "{kobo_label}"
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item shelf-menu-item--danger",
                        "data-testid": "shelf-delete",
                        onclick: move |_| on_delete.call(()),
                        "Delete"
                    }
                }
            }
        }
    }
}
