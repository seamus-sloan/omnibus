//! Web shelf hero: kind / visibility / size above the name, whose shelf it
//! is, its description and rules, and the action bar — Add books, Edit shelf,
//! and a ⋯ menu for Kobo sync and delete. When the viewer can't change the
//! shelf the actions stay where they are, greyed out and inert, above a note
//! that says why.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Navigator};
use omnibus_shared::{MatchMode, Shelf, ShelfKind, UpdateShelfRequest};

use crate::components::shelf_facets::{kind_label, pencil_glyph, rule_text, visibility_label};
use crate::components::shelf_glyphs::{kind_icon, lock_icon, plus_icon, visibility_icon};
use crate::components::user_avatar::UserAvatar;
use crate::components::{confirm_modal_body, ConfirmModal, ConfirmModalAction, ConfirmModalTone};
use crate::shelf_access::{LockReason, ShelfAccess};
use crate::{data, use_server_url, AvatarCacheBust, Route};

/// Id of the lock note every greyed-out action points at.
const LOCK_NOTE_ID: &str = "shd-lock-reason";

/// The hero. `on_add` / `on_edit` open the page's modals; `on_changed` fires
/// after a Kobo toggle so the page refetches.
#[component]
pub(super) fn ShelfHero(
    shelf: Shelf,
    access: ShelfAccess,
    on_add: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let avatar_bust = try_use_context::<AvatarCacheBust>().map_or(0, |b| (b.0)());
    let description = shelf.description.clone().filter(|d| !d.trim().is_empty());
    rsx! {
        header { class: "shd-hero", "data-testid": "shelf-detail-header",
            div { class: "shd-hero-main",
                {kicker(&shelf)}
                h1 { class: "shd-title", "{shelf.name}" }
                {owner_line(&shelf, &access, avatar_bust)}
                if let Some(desc) = description {
                    p { class: "shd-desc", "{desc}" }
                }
                {rules_row(&shelf)}
                if shelf.sync_to_kobo {
                    span {
                        class: "shelf-badge shelf-badge--kobo shd-kobo",
                        "data-testid": "shelf-kobo-badge",
                        "Syncs to Kobo"
                    }
                }
            }
            ShelfActions {
                shelf: shelf.clone(),
                access: access.clone(),
                on_add,
                on_edit,
                on_changed,
            }
            {access_note(&access)}
        }
    }
}

/// Kind, visibility, and size above the title.
fn kicker(shelf: &Shelf) -> Element {
    let count = if shelf.book_count == 1 {
        "1 book".to_string()
    } else {
        format!("{} books", shelf.book_count)
    };
    rsx! {
        div { class: "shd-kicker", "data-testid": "shelf-kicker",
            span { class: "shd-kicker-item shd-kicker-item--kind",
                {kind_icon(shelf.kind)}
                "{kind_label(shelf.kind)}"
            }
            span { class: "shd-kicker-item",
                {visibility_icon(shelf.visibility)}
                "{visibility_label(shelf.visibility)}"
            }
            span { class: "shd-kicker-item", "{count}" }
        }
    }
}

/// The owner's avatar beside whose shelf this is, relative to the viewer.
fn owner_line(shelf: &Shelf, access: &ShelfAccess, avatar_bust: u32) -> Element {
    let own = matches!(
        access,
        ShelfAccess::Owner | ShelfAccess::Locked(LockReason::System { own: true, .. })
    );
    rsx! {
        div { class: "shd-owner", "data-testid": "shelf-owner",
            span { class: "shd-owner-av", aria_hidden: "true",
                UserAvatar {
                    user_id: shelf.owner_user_id,
                    name: shelf.owner_username.clone(),
                    has_avatar: shelf.owner_has_avatar,
                    class: "shv-av",
                    bust: avatar_bust,
                }
            }
            if own {
                span { class: "shd-owner-name", "Your shelf" }
            } else {
                span { class: "shd-owner-lead", "Owned by" }
                span { class: "shd-owner-name", "data-testid": "shelf-owner-name", "{shelf.owner_username}" }
            }
        }
    }
}

/// A smart shelf's rules, as the sentence they add up to.
fn rules_row(shelf: &Shelf) -> Element {
    if shelf.kind != ShelfKind::Smart || shelf.rules.is_empty() {
        return rsx! {};
    }
    let lead = match shelf.match_mode.unwrap_or(MatchMode::All) {
        MatchMode::All => "Books matching all of",
        MatchMode::Any => "Books matching any of",
    };
    rsx! {
        div { class: "shd-rules", "data-testid": "shelf-rules",
            span { class: "shd-rules-lead", "{lead}" }
            for (i, rule) in shelf.rules.iter().enumerate() {
                span { key: "{i}", class: "shelf-rule-chip", "{rule_text(rule)}" }
            }
        }
    }
}

/// The note under the actions: why they're greyed out, when they are.
///
/// Nothing is said to a viewer who *may* edit, an admin changing someone
/// else's shelf included. A permission note earns its place only by explaining
/// a refusal — and the hero already names the owner either way.
fn access_note(access: &ShelfAccess) -> Element {
    match access {
        ShelfAccess::Locked(reason) => rsx! {
            div {
                class: "shd-note shd-note--locked",
                id: LOCK_NOTE_ID,
                role: "note",
                "data-testid": "shelf-lock-reason",
                span { class: "shd-note-icon", aria_hidden: "true", {lock_icon()} }
                div { class: "shd-note-text",
                    strong { "{reason.headline()}" }
                    span { "{reason.detail()}" }
                }
            }
        },
        ShelfAccess::Owner | ShelfAccess::Admin | ShelfAccess::Pending => rsx! {},
    }
}

/// The attributes every action shares, so a greyed-out control is one the
/// viewer can still focus and hear the reason for, but can't trigger.
struct ActionAttrs {
    disabled: &'static str,
    describedby: Option<&'static str>,
    tip: Option<String>,
}

impl ActionAttrs {
    fn for_access(access: &ShelfAccess) -> Self {
        let reason = access.lock_reason();
        Self {
            disabled: if access.can_edit() { "false" } else { "true" },
            describedby: reason.map(|_| LOCK_NOTE_ID),
            tip: reason.map(LockReason::headline),
        }
    }
}

/// The action bar. Every viewer gets the same controls for the shelf's kind —
/// Add books (hand-picked only), Edit shelf, the ⋯ menu — and when `access`
/// can't edit, each is `aria-disabled`, inert, and described by the lock note.
#[component]
fn ShelfActions(
    shelf: Shelf,
    access: ShelfAccess,
    on_add: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let mut menu_open = use_signal(|| false);
    let delete = DeleteSignals {
        open: use_signal(|| false),
        busy: use_signal(|| false),
        error: use_signal(|| None::<String>),
    };

    let can_edit = access.can_edit();
    let attrs = ActionAttrs::for_access(&access);
    let is_manual = shelf.kind == ShelfKind::Manual;
    let on_toggle_kobo = build_on_toggle_kobo(
        server_url.clone(),
        shelf.id,
        !shelf.sync_to_kobo,
        menu_open,
        on_changed,
    );
    let mut open_delete = delete.open;
    let on_delete = EventHandler::new(move |()| {
        menu_open.set(false);
        open_delete.set(true);
    });

    rsx! {
        div { class: "shd-actions", role: "group", "aria-label": "Shelf actions",
            if is_manual {
                button {
                    r#type: "button",
                    class: "btn primary shd-action",
                    "data-testid": "shelf-add-books",
                    "aria-disabled": attrs.disabled,
                    "aria-describedby": attrs.describedby,
                    title: attrs.tip.clone(),
                    onclick: move |_| if can_edit { on_add.call(()) },
                    {plus_icon()}
                    "Add books"
                }
            }
            button {
                r#type: "button",
                class: if is_manual { "btn shd-action" } else { "btn primary shd-action" },
                "data-testid": "shelf-edit",
                "aria-disabled": attrs.disabled,
                "aria-describedby": attrs.describedby,
                title: attrs.tip.clone(),
                onclick: move |_| if can_edit { on_edit.call(()) },
                {pencil_glyph()}
                "Edit shelf"
            }
            div { class: "shd-more",
                button {
                    r#type: "button",
                    class: "btn shd-action shd-more-btn",
                    "data-testid": "shelf-actions",
                    "aria-label": "More shelf actions",
                    "aria-haspopup": "menu",
                    "aria-expanded": if menu_open() { "true" } else { "false" },
                    "aria-disabled": attrs.disabled,
                    "aria-describedby": attrs.describedby,
                    title: attrs.tip.clone(),
                    onclick: move |_| if can_edit { menu_open.toggle() },
                    "\u{22EF}"
                }
                if can_edit && menu_open() {
                    {more_menu(shelf.sync_to_kobo, menu_open, on_toggle_kobo, on_delete)}
                }
            }
        }
        if (delete.open)() {
            {render_delete_modal(server_url, shelf.id, shelf.name.clone(), nav, delete)}
        }
    }
}

/// The ⋯ dropdown — Kobo sync and delete — over a transparent catcher that
/// closes it on any outside click.
fn more_menu(
    syncs_to_kobo: bool,
    mut menu_open: Signal<bool>,
    on_toggle_kobo: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "shd-menu-scrim", onclick: move |_| menu_open.set(false) }
        div { class: "shelf-actions-menu shd-menu", role: "menu", "aria-label": "More shelf actions",
            button {
                r#type: "button",
                class: "shelf-menu-item shd-menu-item",
                role: "menuitemcheckbox",
                "aria-checked": if syncs_to_kobo { "true" } else { "false" },
                "data-testid": "shelf-toggle-kobo",
                onclick: move |_| on_toggle_kobo.call(()),
                span { class: "shd-menu-check", aria_hidden: "true",
                    if syncs_to_kobo { "\u{2713}" }
                }
                "Sync to Kobo"
            }
            button {
                r#type: "button",
                class: "shelf-menu-item shelf-menu-item--danger shd-menu-item",
                role: "menuitem",
                "data-testid": "shelf-delete",
                onclick: move |_| on_delete.call(()),
                span { class: "shd-menu-check", aria_hidden: "true" }
                "Delete shelf"
            }
        }
    }
}

/// The delete confirmation's state: shown, in flight, and the failure to report.
#[derive(Clone, Copy)]
struct DeleteSignals {
    open: Signal<bool>,
    busy: Signal<bool>,
    error: Signal<Option<String>>,
}

/// The delete-shelf confirmation. Can't be dismissed mid-delete; a success
/// returns to the shelves index, and a failure stays here and says so.
fn render_delete_modal(
    server_url: String,
    id: i64,
    shelf_name: String,
    nav: Navigator,
    sig: DeleteSignals,
) -> Element {
    let DeleteSignals {
        mut open,
        mut busy,
        mut error,
    } = sig;
    let is_busy = busy();
    let do_delete = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        error.set(None);
        let url = server_url.clone();
        spawn(async move {
            match data::delete_shelf(&url, id).await {
                Ok(()) => {
                    nav.push(Route::Shelves {});
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    busy.set(false);
                }
            }
        });
    };
    rsx! {
        ConfirmModal {
            testid: "shelf-delete-modal".to_string(),
            aria_label: "Delete shelf?".to_string(),
            dialog_class: "mg-modal del-modal".to_string(),
            busy: is_busy,
            on_dismiss: move |_| open.set(false),
            {confirm_modal_body(
                "Delete shelf?",
                &format!(
                    "Deleting \u{201c}{shelf_name}\u{201d} removes the shelf. Its books stay in your library. This can\u{2019}t be undone."
                ),
                vec![
                    ConfirmModalAction {
                        testid: "shelf-delete-cancel".to_string(),
                        label: "Cancel".to_string(),
                        tone: ConfirmModalTone::Ghost,
                        disabled: is_busy,
                        on_click: EventHandler::new(move |_| open.set(false)),
                    },
                    ConfirmModalAction {
                        testid: "shelf-delete-confirm".to_string(),
                        label: if is_busy { "Deleting\u{2026}".to_string() } else { "Delete".to_string() },
                        tone: ConfirmModalTone::Danger,
                        disabled: is_busy,
                        on_click: EventHandler::new(do_delete),
                    },
                ],
            )}
            if let Some(msg) = error() {
                p { role: "alert", class: "shelf-modal-error", "data-testid": "shelf-delete-error",
                    "Couldn\u{2019}t delete this shelf: {msg}"
                }
            }
        }
    }
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
