//! The New / Edit / Delete user modals, plus the shared `ModalShell` /
//! `ModalError` chrome they build on. Split out of `users` because they
//! share nothing with the table/row markup beyond `PermissionToggles`.

use dioxus::prelude::*;
use omnibus_shared::{AdminUserRow, CreateUserRequest, UserPermissions};

use crate::components::auth::{score_password, PasswordRequirements, StrengthMeter};
use crate::components::{confirm_modal_body, ConfirmModal, ConfirmModalAction, ConfirmModalTone};
use crate::data;

use super::PermissionToggles;

/// New-user modal: username + password (with strength meter/checklist) +
/// permissions.
#[component]
pub(super) fn NewUserModal(on_close: EventHandler<()>, on_created: EventHandler<()>) -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let perms = use_signal(|| UserPermissions {
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
    });
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    let submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        if saving() {
            return;
        }
        let u = username().trim().to_string();
        let pw = password();
        if u.is_empty() || pw.is_empty() {
            error.set(Some("Enter a username and password.".into()));
            return;
        }
        saving.set(true);
        error.set(None);
        let req = CreateUserRequest {
            username: u,
            password: pw,
            permissions: perms(),
        };
        spawn(async move {
            match data::create_user(req).await {
                Ok(_) => on_created.call(()),
                Err(e) => error.set(Some(e)),
            }
            saving.set(false);
        });
    };

    let pw = password();
    let (score, score_label, rules) = score_password(&pw);

    rsx! {
        ModalShell { title: "New user", testid: "users-new-modal", on_close,
            form { class: "settings-form", onsubmit: submit,
                div { class: "settings-field",
                    label { r#for: "new-user-username", "Username" }
                    input {
                        r#type: "text",
                        id: "new-user-username",
                        "data-testid": "new-user-username",
                        autocomplete: "off",
                        autocapitalize: "none",
                        spellcheck: "false",
                        value: "{username}",
                        oninput: move |e| { username.set(e.value()); error.set(None); },
                    }
                }
                div { class: "settings-field",
                    label { r#for: "new-user-password", "Password" }
                    input {
                        r#type: "password",
                        id: "new-user-password",
                        "data-testid": "new-user-password",
                        autocomplete: "new-password",
                        value: "{password}",
                        oninput: move |e| { password.set(e.value()); error.set(None); },
                    }
                }
                StrengthMeter { score, label: Some(score_label.to_string()) }
                PasswordRequirements { rules }
                PermissionToggles { perms }
                ModalError { error: error() }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        "data-testid": "new-user-submit",
                        disabled: saving(),
                        "Create user"
                    }
                    button { r#type: "button", class: "btn ghost", onclick: move |_| on_close.call(()), "Cancel" }
                }
            }
        }
    }
}

/// Edit-user modal: permission toggles + an optional password reset.
#[component]
pub(super) fn EditUserModal(
    user: AdminUserRow,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let uid = user.id;
    let perms = use_signal(|| UserPermissions {
        is_admin: user.is_admin,
        can_upload: user.can_upload,
        can_edit: user.can_edit,
        can_download: user.can_download,
    });
    let mut new_password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    let submit = move |evt: Event<FormData>| {
        evt.prevent_default();
        if saving() {
            return;
        }
        saving.set(true);
        error.set(None);
        let perms_val = perms();
        let pw = new_password().trim().to_string();
        spawn(async move {
            if let Err(e) = data::update_permissions(uid, perms_val).await {
                error.set(Some(e));
                saving.set(false);
                return;
            }
            if !pw.is_empty() {
                if let Err(e) = data::set_password(uid, pw).await {
                    error.set(Some(e));
                    saving.set(false);
                    return;
                }
            }
            on_saved.call(());
            saving.set(false);
        });
    };

    let pw = new_password();
    let show_meter = !pw.is_empty();
    let (score, score_label, rules) = score_password(&pw);

    rsx! {
        ModalShell { title: "Edit {user.username}", testid: "users-edit-modal", on_close,
            form { class: "settings-form", onsubmit: submit,
                PermissionToggles { perms }
                div { class: "settings-field",
                    label { r#for: "edit-user-password", "Reset password" }
                    input {
                        r#type: "password",
                        id: "edit-user-password",
                        "data-testid": "edit-user-password",
                        autocomplete: "new-password",
                        placeholder: "Leave blank to keep current",
                        value: "{new_password}",
                        oninput: move |e| { new_password.set(e.value()); error.set(None); },
                    }
                }
                if show_meter {
                    StrengthMeter { score, label: Some(score_label.to_string()) }
                    PasswordRequirements { rules }
                }
                ModalError { error: error() }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        "data-testid": "edit-user-submit",
                        disabled: saving(),
                        "Save changes"
                    }
                    button { r#type: "button", class: "btn ghost", onclick: move |_| on_close.call(()), "Cancel" }
                }
            }
        }
    }
}

/// Delete-confirmation modal. Warns when the admin is deleting their own
/// account; the last-admin guard is enforced server-side and surfaced inline.
/// Built on the shared `ConfirmModal` shell (see `components::confirm_modal`)
/// rather than `ModalShell`, so the backdrop can't be dismissed mid-delete —
/// `ModalShell`'s own backdrop has no busy gate at all.
#[component]
pub(super) fn DeleteUserModal(
    user: AdminUserRow,
    is_self: bool,
    on_close: EventHandler<()>,
    on_deleted: EventHandler<()>,
) -> Element {
    let uid = user.id;
    let mut error = use_signal(|| None::<String>);
    let mut deleting = use_signal(|| false);
    let busy = deleting();
    let title = format!("Delete {}", user.username);
    let body = format!(
        "Permanently delete {} and all of their reading data. This can't be undone.",
        user.username
    );

    let confirm = move |_| {
        if deleting() {
            return;
        }
        deleting.set(true);
        error.set(None);
        spawn(async move {
            match data::delete_user(uid).await {
                Ok(()) => on_deleted.call(()),
                Err(e) => error.set(Some(e)),
            }
            deleting.set(false);
        });
    };

    rsx! {
        ConfirmModal {
            testid: "users-delete-modal".to_string(),
            aria_label: title.clone(),
            dialog_class: "users-modal-card".to_string(),
            busy,
            on_dismiss: move |_| on_close.call(()),
            if is_self {
                p { class: "settings-status error", "data-testid": "delete-self-warning",
                    "This is your own account — you'll be signed out."
                }
            }
            ModalError { error: error() }
            {confirm_modal_body(
                &title,
                &body,
                vec![
                    ConfirmModalAction {
                        testid: "delete-user-cancel".to_string(),
                        label: "Cancel".to_string(),
                        tone: ConfirmModalTone::Ghost,
                        disabled: busy,
                        on_click: EventHandler::new(move |_| on_close.call(())),
                    },
                    ConfirmModalAction {
                        testid: "delete-user-confirm".to_string(),
                        label: if busy { "Deleting\u{2026}".to_string() } else { "Delete user".to_string() },
                        tone: ConfirmModalTone::Danger,
                        disabled: busy,
                        on_click: EventHandler::new(confirm),
                    },
                ],
            )}
        }
    }
}

/// Shared modal overlay + card chrome. Clicking the scrim or the close button
/// dismisses; clicks inside the card don't propagate.
#[component]
fn ModalShell(
    title: String,
    testid: String,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "users-modal-overlay",
            "data-testid": "{testid}",
            onclick: move |_| on_close.call(()),
            div {
                class: "users-modal-card",
                role: "dialog",
                "aria-modal": "true",
                onclick: move |e| e.stop_propagation(),
                div { class: "users-modal-head",
                    h3 { "{title}" }
                    button {
                        r#type: "button",
                        class: "users-modal-close",
                        "aria-label": "Close",
                        onclick: move |_| on_close.call(()),
                        "\u{00d7}"
                    }
                }
                div { class: "users-modal-body", {children} }
            }
        }
    }
}

/// Inline error row shared by the modals.
#[component]
fn ModalError(error: Option<String>) -> Element {
    match error {
        Some(msg) => rsx! {
            p { role: "alert", class: "settings-status error", "data-testid": "users-modal-error", "{msg}" }
        },
        None => rsx! {},
    }
}
