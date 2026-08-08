//! The New / Edit / Delete user modals, plus the shared `ModalShell` /
//! `ModalError` chrome they build on. Split out of `users` because they
//! share nothing with the table/row markup beyond `PermissionToggles`.

use dioxus::prelude::*;
use omnibus_shared::{AdminUserRow, CreateUserRequest, DeviceView, SessionView, UserPermissions};

use crate::components::auth::{score_password, PasswordRequirements, StrengthMeter};
use crate::components::{confirm_modal_body, ConfirmModal, ConfirmModalAction, ConfirmModalTone};
use crate::data;

use super::PermissionToggles;

/// New-user modal: username + password (with strength meter/checklist) +
/// permissions.
#[component]
pub(super) fn NewUserModal(on_close: EventHandler<()>, on_created: EventHandler<()>) -> Element {
    let username = use_signal(String::new);
    let password = use_signal(String::new);
    let perms = use_signal(|| UserPermissions {
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
    });
    let error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);
    let submit =
        build_new_user_submit_handler(username, password, perms, error, saving, on_created);

    rsx! {
        ModalShell { title: "New user", testid: "users-new-modal", on_close,
            form { class: "settings-form", onsubmit: submit,
                NewUserFields { username, password, error }
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

/// Username + password inputs with the shared strength meter/checklist.
#[component]
fn NewUserFields(
    mut username: Signal<String>,
    mut password: Signal<String>,
    mut error: Signal<Option<String>>,
) -> Element {
    let pw = password();
    let (score, score_label, rules) = score_password(&pw);
    rsx! {
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
    }
}

/// Builds the new-user submit handler: local validation, then
/// `data::create_user`.
fn build_new_user_submit_handler(
    username: Signal<String>,
    password: Signal<String>,
    perms: Signal<UserPermissions>,
    mut error: Signal<Option<String>>,
    mut saving: Signal<bool>,
    on_created: EventHandler<()>,
) -> impl FnMut(Event<FormData>) + 'static {
    move |evt: Event<FormData>| {
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

/// Device & session management modal (F5.4, #910): lists a target user's
/// live sessions and registered devices, each with a Revoke action. Both
/// lists reload after any revoke so the rendered state always matches the
/// server. Devices are listed separately from sessions — revoking a device
/// also revokes any session still tied to it (enforced server-side), so a
/// device row disappearing can also drop a session row on the next reload.
#[component]
pub(super) fn SessionsModal(user: AdminUserRow, on_close: EventHandler<()>) -> Element {
    let uid = user.id;
    let mut sessions = use_signal(Vec::<SessionView>::new);
    let mut devices = use_signal(Vec::<DeviceView>::new);
    let mut error = use_signal(|| None::<String>);
    let mut reload = use_signal(|| 0u32);
    let mut busy_id = use_signal(|| None::<i64>);

    use_effect(move || {
        let _ = reload();
        // Clear any error left over from a previous fetch/revoke *before* this
        // reload starts, not just on failure — otherwise a stale banner from an
        // earlier transient failure keeps showing even once this reload succeeds.
        error.set(None);
        spawn(async move {
            match data::list_user_sessions(uid).await {
                Ok(rows) => sessions.set(rows),
                Err(e) => error.set(Some(e)),
            }
            match data::list_user_devices(uid).await {
                Ok(rows) => devices.set(rows),
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let revoke_session = use_callback(move |session_id: i64| {
        if busy_id().is_some() {
            return;
        }
        busy_id.set(Some(session_id));
        error.set(None);
        spawn(async move {
            match data::revoke_user_session(session_id).await {
                Ok(()) => reload.with_mut(|n| *n += 1),
                Err(e) => error.set(Some(e)),
            }
            busy_id.set(None);
        });
    });
    let revoke_device = use_callback(move |device_id: i64| {
        if busy_id().is_some() {
            return;
        }
        busy_id.set(Some(device_id));
        error.set(None);
        spawn(async move {
            match data::revoke_user_device(device_id).await {
                Ok(()) => reload.with_mut(|n| *n += 1),
                Err(e) => error.set(Some(e)),
            }
            busy_id.set(None);
        });
    });

    rsx! {
        ModalShell {
            title: "Sessions & devices \u{2014} {user.username}",
            testid: "users-sessions-modal",
            on_close,
            ModalError { error: error() }
            h4 { "Sessions" }
            {render_session_list(&sessions(), busy_id(), revoke_session)}
            h4 { "Devices" }
            {render_device_list(&devices(), busy_id(), revoke_device)}
            div { class: "settings-actions",
                button { r#type: "button", class: "btn ghost", onclick: move |_| on_close.call(()), "Close" }
            }
        }
    }
}

/// The session list, or an empty-state line when the user has none live.
fn render_session_list(
    rows: &[SessionView],
    busy_id: Option<i64>,
    on_revoke: Callback<i64>,
) -> Element {
    if rows.is_empty() {
        return rsx! {
            p { class: "settings-status", "data-testid": "user-sessions-empty", "No live sessions." }
        };
    }
    rsx! {
        ul { class: "users-session-list", "data-testid": "user-sessions-list",
            for s in rows {
                {
                    let id = s.id;
                    let kind = s.kind.clone();
                    let last_used = super::fmt_date(s.last_used_at);
                    rsx! {
                        li { key: "{id}", class: "users-session-row", "data-testid": "user-session-row",
                            span { "{kind} \u{00b7} last used {last_used}" }
                            button {
                                r#type: "button",
                                class: "btn ghost danger sm",
                                "data-testid": "user-session-revoke-{id}",
                                disabled: busy_id == Some(id),
                                onclick: move |_| on_revoke.call(id),
                                "Revoke"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The device list, or an empty-state line when the user has none registered.
fn render_device_list(
    rows: &[DeviceView],
    busy_id: Option<i64>,
    on_revoke: Callback<i64>,
) -> Element {
    if rows.is_empty() {
        return rsx! {
            p { class: "settings-status", "data-testid": "user-devices-empty", "No registered devices." }
        };
    }
    rsx! {
        ul { class: "users-session-list", "data-testid": "user-devices-list",
            for d in rows {
                {
                    let id = d.id;
                    let last_seen = super::fmt_date(d.last_seen_at);
                    rsx! {
                        li { key: "{id}", class: "users-session-row", "data-testid": "user-device-row",
                            span { "{d.name} \u{00b7} {d.client_kind} \u{00b7} last seen {last_seen}" }
                            button {
                                r#type: "button",
                                class: "btn ghost danger sm",
                                "data-testid": "user-device-revoke-{id}",
                                disabled: busy_id == Some(id),
                                onclick: move |_| on_revoke.call(id),
                                "Revoke"
                            }
                        }
                    }
                }
            }
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
