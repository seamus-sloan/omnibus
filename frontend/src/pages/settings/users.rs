//! Users settings section (F5.4) — the admin table plus the New / Edit /
//! Delete modals and inline Unlock. Admin-only; rendered as the `users`
//! section of `/settings`. SSR and the first WASM paint both start from an
//! empty list (rule 07); the post-mount effect loads the real rows.

use dioxus::prelude::*;
use omnibus_shared::{AdminUserRow, CreateUserRequest, UserPermissions};

use crate::components::auth::{score_password, PasswordRequirements, StrengthMeter};
use crate::data;

/// Which modal, if any, is open over the Users table.
#[derive(Clone, PartialEq)]
enum Modal {
    None,
    New,
    Edit(AdminUserRow),
    Delete(AdminUserRow),
}

/// The Users section: header + table, with a reload counter the mutations bump.
#[component]
pub fn UsersSection() -> Element {
    let mut users = use_signal(Vec::<AdminUserRow>::new);
    let mut load_error = use_signal(|| None::<String>);
    let mut reload = use_signal(|| 0u32);
    let mut modal = use_signal(|| Modal::None);
    let current = crate::use_current_user_summary();

    // Reload whenever the counter bumps (initial mount + after each mutation).
    use_effect(move || {
        let _ = reload();
        spawn(async move {
            match data::list_users().await {
                Ok(rows) => {
                    users.set(rows);
                    load_error.set(None);
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    });

    let current_id = current().map(|u| u.id);

    rsx! {
        section { class: "card", "data-testid": "users-card",
            div { class: "users-head",
                div {
                    h2 { "Users" }
                    p { class: "subtitle", "Create, edit, and remove accounts." }
                }
                button {
                    r#type: "button",
                    class: "btn",
                    "data-testid": "users-new",
                    onclick: move |_| modal.set(Modal::New),
                    "New user"
                }
            }

            if let Some(err) = load_error() {
                p { role: "alert", class: "settings-status error", "data-testid": "users-load-error", "{err}" }
            }

            table { class: "users-table", "data-testid": "users-table",
                thead {
                    tr {
                        th { "User" }
                        th { "Permissions" }
                        th { "Kindle" }
                        th { "Created" }
                        th { "Status" }
                        th { class: "users-col-actions", "Actions" }
                    }
                }
                tbody {
                    for u in users() {
                        UserRow {
                            key: "{u.id}",
                            user: u.clone(),
                            is_self: Some(u.id) == current_id,
                            on_edit: move |row| modal.set(Modal::Edit(row)),
                            on_delete: move |row| modal.set(Modal::Delete(row)),
                            on_unlocked: move |_| { reload.with_mut(|n| *n += 1); },
                        }
                    }
                }
            }
        }

        match modal() {
            Modal::None => rsx! {},
            Modal::New => rsx! {
                NewUserModal {
                    on_close: move |_| modal.set(Modal::None),
                    on_created: move |_| { modal.set(Modal::None); reload.with_mut(|n| *n += 1); },
                }
            },
            Modal::Edit(row) => rsx! {
                EditUserModal {
                    user: row,
                    on_close: move |_| modal.set(Modal::None),
                    on_saved: move |_| { modal.set(Modal::None); reload.with_mut(|n| *n += 1); },
                }
            },
            Modal::Delete(row) => {
                let is_self = current_id == Some(row.id);
                rsx! {
                    DeleteUserModal {
                        user: row,
                        is_self,
                        on_close: move |_| modal.set(Modal::None),
                        on_deleted: move |_| { modal.set(Modal::None); reload.with_mut(|n| *n += 1); },
                    }
                }
            }
        }
    }
}

/// One table row: identity, permission chips, Kindle, created date, lock
/// status (with inline Unlock), and Edit / Delete actions.
#[component]
fn UserRow(
    user: AdminUserRow,
    is_self: bool,
    on_edit: EventHandler<AdminUserRow>,
    on_delete: EventHandler<AdminUserRow>,
    on_unlocked: EventHandler<()>,
) -> Element {
    let mut unlocking = use_signal(|| false);
    let uid = user.id;
    let kindle = user
        .kindle_email
        .clone()
        .unwrap_or_else(|| "\u{2014}".into());

    let unlock = move |_| {
        if unlocking() {
            return;
        }
        unlocking.set(true);
        spawn(async move {
            let _ = data::unlock_user(uid).await;
            unlocking.set(false);
            on_unlocked.call(());
        });
    };

    let edit_row = user.clone();
    let delete_row = user.clone();

    rsx! {
        tr { "data-testid": "user-row-{uid}",
            td {
                span { class: "users-name", "{user.username}" }
                if is_self { span { class: "users-self-tag", " (you)" } }
            }
            td { {permission_chips(&user)} }
            td { class: "users-kindle", "{kindle}" }
            td { "{fmt_date(user.created_at)}" }
            td {
                if user.locked {
                    span { class: "users-badge users-badge-locked", "Locked" }
                    button {
                        r#type: "button",
                        class: "btn ghost sm",
                        "data-testid": "user-unlock-{uid}",
                        disabled: unlocking(),
                        onclick: unlock,
                        "Unlock"
                    }
                } else {
                    span { class: "users-badge", "Active" }
                }
            }
            td { class: "users-col-actions",
                button {
                    r#type: "button",
                    class: "btn ghost sm",
                    "data-testid": "user-edit-{uid}",
                    onclick: move |_| on_edit.call(edit_row.clone()),
                    "Edit"
                }
                button {
                    r#type: "button",
                    class: "btn ghost sm users-danger",
                    "data-testid": "user-delete-{uid}",
                    onclick: move |_| on_delete.call(delete_row.clone()),
                    "Delete"
                }
            }
        }
    }
}

/// The four permission checkboxes. `is_admin` ("Administrator") implies the
/// other three: checking it forces them on and disables them, mirroring the
/// storage rule that an admin can do everything (there is no role enum).
#[component]
fn PermissionToggles(perms: Signal<UserPermissions>) -> Element {
    let p = perms();
    let admin = p.is_admin;
    rsx! {
        fieldset { class: "users-perms",
            legend { "Permissions" }
            PermissionCheckbox {
                label: "Administrator",
                testid: "perm-is_admin",
                checked: p.is_admin,
                disabled: false,
                on_toggle: move |v: bool| perms.with_mut(|pp| {
                    pp.is_admin = v;
                    if v { pp.can_upload = true; pp.can_edit = true; pp.can_download = true; }
                }),
            }
            PermissionCheckbox {
                label: "Upload books",
                testid: "perm-can_upload",
                checked: p.can_upload,
                disabled: admin,
                on_toggle: move |v: bool| perms.with_mut(|pp| pp.can_upload = v),
            }
            PermissionCheckbox {
                label: "Edit metadata",
                testid: "perm-can_edit",
                checked: p.can_edit,
                disabled: admin,
                on_toggle: move |v: bool| perms.with_mut(|pp| pp.can_edit = v),
            }
            PermissionCheckbox {
                label: "Download",
                testid: "perm-can_download",
                checked: p.can_download,
                disabled: admin,
                on_toggle: move |v: bool| perms.with_mut(|pp| pp.can_download = v),
            }
        }
    }
}

#[component]
fn PermissionCheckbox(
    label: String,
    testid: String,
    checked: bool,
    disabled: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "users-perm-row",
            input {
                r#type: "checkbox",
                "data-testid": "{testid}",
                checked,
                disabled,
                oninput: move |e| on_toggle.call(e.value() == "true"),
            }
            span { "{label}" }
        }
    }
}

/// New-user modal: username + password (with strength meter/checklist) +
/// permissions.
#[component]
fn NewUserModal(on_close: EventHandler<()>, on_created: EventHandler<()>) -> Element {
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
fn EditUserModal(
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
#[component]
fn DeleteUserModal(
    user: AdminUserRow,
    is_self: bool,
    on_close: EventHandler<()>,
    on_deleted: EventHandler<()>,
) -> Element {
    let uid = user.id;
    let mut error = use_signal(|| None::<String>);
    let mut deleting = use_signal(|| false);

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
        ModalShell { title: "Delete {user.username}", testid: "users-delete-modal", on_close,
            p {
                "Permanently delete "
                b { "{user.username}" }
                " and all of their reading data. This can't be undone."
            }
            if is_self {
                p { class: "settings-status error", "data-testid": "delete-self-warning",
                    "This is your own account — you'll be signed out."
                }
            }
            ModalError { error: error() }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn users-danger",
                    "data-testid": "delete-user-confirm",
                    disabled: deleting(),
                    onclick: confirm,
                    "Delete user"
                }
                button { r#type: "button", class: "btn ghost", onclick: move |_| on_close.call(()), "Cancel" }
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

// ── Pure helpers ─────────────────────────────────────────────────

/// Permission chips for a row: a single "Admin" chip when `is_admin`,
/// otherwise one chip per granted capability (or "—" when none).
fn permission_chips(user: &AdminUserRow) -> Element {
    if user.is_admin {
        return rsx! { span { class: "users-chip users-chip-admin", "Admin" } };
    }
    let mut chips: Vec<&str> = Vec::new();
    if user.can_upload {
        chips.push("Upload");
    }
    if user.can_edit {
        chips.push("Edit");
    }
    if user.can_download {
        chips.push("Download");
    }
    if chips.is_empty() {
        return rsx! { span { class: "users-chip-none", "\u{2014}" } };
    }
    rsx! {
        for c in chips {
            span { key: "{c}", class: "users-chip", "{c}" }
        }
    }
}

/// Format a Unix-seconds timestamp as `Mon D, YYYY` (UTC). Pure so SSR and
/// the hydrated client render byte-identical text.
fn fmt_date(unix_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = civil_from_days(unix_secs.div_euclid(86_400));
    let name = MONTHS
        .get((m as usize).saturating_sub(1))
        .copied()
        .unwrap_or("");
    format!("{name} {d}, {y}")
}

/// Days since the Unix epoch → `(year, month, day)` civil date (Howard
/// Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests;
