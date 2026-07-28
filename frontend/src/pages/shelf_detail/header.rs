//! Shelf detail header: back link, the name with its edit pencil, the facet
//! row (kind / visibility / rule chips, plus Kobo + owner badges), and the
//! owner-only actions menu (toggle Kobo sync, delete).

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{Shelf, ShelfKind, UpdateShelfRequest};

use crate::components::shelf_facets::pencil_glyph;
use crate::components::ShelfFacets;
use crate::{data, use_server_url, Route};

/// Header: back link, name + edit pencil, facet row, and the actions menu.
/// `on_changed` fires after a successful Kobo-sync toggle so the parent
/// refetches; `on_edit` opens the edit-shelf modal (name, visibility, and —
/// for smart shelves — rules).
#[component]
pub(super) fn ShelfHeader(
    shelf: Shelf,
    on_edit: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let menu_open = use_signal(|| false);

    // Owner/admin gating; `None` until the boot effect resolves the viewer, so
    // controls stay hidden on SSR + first paint (hydration parity, rule 07). The
    // server enforces the same rule (`shelf_for_edit`).
    let viewer = crate::use_current_user_summary()();
    let owner_id = shelf.owner_user_id;
    // A system shelf (Wishlist) is locked even for its owner/admin: no edit,
    // delete, or Kobo toggle. Mirrors the server-side `ShelfError::SystemShelf`
    // rejection so the UI never offers a 403 action.
    let can_manage = !shelf.kind.is_system()
        && viewer
            .as_ref()
            .is_some_and(|u| u.id == owner_id || u.is_admin);
    // Suppressed on a wishlist: its name already opens with the owner's username.
    let show_attribution =
        viewer.as_ref().is_some_and(|u| u.id != owner_id) && shelf.kind != ShelfKind::Wishlist;

    let id = shelf.id;
    let syncs_to_kobo = shelf.sync_to_kobo;

    let on_delete = build_on_delete(server_url.clone(), id, nav);
    let on_toggle_kobo = build_on_toggle_kobo(
        server_url.clone(),
        id,
        !syncs_to_kobo,
        menu_open,
        on_changed,
    );

    rsx! {
        header { class: "shelf-detail-header", "data-testid": "shelf-detail-header",
            Link { to: Route::Landing {}, class: "shelf-back", "\u{2190} All books" }
            div { class: "shelf-title-row",
                h1 { class: "shelf-title", "{shelf.name}" }
                if can_manage {
                    button {
                        r#type: "button",
                        class: "shelf-edit-btn",
                        "data-testid": "shelf-edit",
                        "aria-label": "Edit shelf",
                        onclick: move |_| on_edit.call(()),
                        {pencil_glyph()}
                    }
                    {shelf_actions_menu(
                        syncs_to_kobo,
                        menu_open,
                        ShelfMenuActions {
                            toggle_kobo: on_toggle_kobo,
                            delete: on_delete,
                        },
                    )}
                }
            }
            ShelfFacets { shelf: shelf.clone(),
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

/// The actions-menu handlers, bundled so [`shelf_actions_menu`] stays inside
/// clippy's argument-count limit.
struct ShelfMenuActions {
    toggle_kobo: EventHandler<()>,
    delete: EventHandler<()>,
}

/// Owner-only actions trigger + dropdown: toggle Kobo sync, delete. Name,
/// visibility, and rules are edited via the title-row pencil's edit-shelf
/// modal instead. Split out of [`ShelfHeader`] to keep it under the line
/// cap, mirroring `body::member_grid`'s plain-fn split.
fn shelf_actions_menu(
    syncs_to_kobo: bool,
    mut menu_open: Signal<bool>,
    actions: ShelfMenuActions,
) -> Element {
    let ShelfMenuActions {
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
