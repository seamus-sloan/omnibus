//! Profile card at the top of Settings → Account: display name + avatar.
//!
//! Both writes are account configuration, so they go straight to the server
//! and surface failures inline — never queued (rule 08). Signals start empty so
//! SSR and the first WASM paint agree (rule 07); the hydration effect fills
//! them from the resolved user.

use dioxus::prelude::*;

use crate::components::credential_card::credential_status_message;
use crate::components::image_upload::use_file_upload;
use crate::components::user_avatar::UserAvatar;
use crate::{data, use_server_url};

/// Display name + profile picture for the signed-in user.
///
/// On a successful write it re-fetches `/api/auth/me` into the app-wide
/// `CurrentUser` context, so the user menu, journal bylines and the mobile
/// "You" tab all repaint without a reload, and bumps the avatar cache-bust so
/// the replaced image (same URL, new bytes) is actually re-fetched.
#[component]
pub(crate) fn ProfileCard() -> Element {
    let server_url = use_server_url();
    let current_user = crate::use_current_user().0;
    let mut bust = crate::use_avatar_cache_bust().0;

    let mut name_input = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);
    let mut uploading = use_signal(|| false);
    let mut upload_error = use_signal(|| None::<String>);
    // Seeded once from the resolved user; a later refresh must not clobber
    // what the user is mid-way through typing.
    let mut seeded = use_signal(|| false);

    let user = current_user().flatten();

    use_effect(move || {
        if seeded() {
            return;
        }
        if let Some(u) = current_user().flatten() {
            name_input.set(u.display_name.clone().unwrap_or_default());
            seeded.set(true);
        }
    });

    // Re-read `/api/auth/me` so every avatar/name site repaints from one
    // source rather than each guessing at the new value.
    let refresh_user = move || {
        spawn(async move {
            let mut cu = current_user;
            if let Ok(u) = data::current_user().await {
                cu.set(Some(u));
            }
        });
    };

    let on_save = {
        let server_url = server_url.clone();
        move |evt: Event<FormData>| {
            evt.prevent_default();
            let trimmed = name_input().trim().to_string();
            let value = (!trimmed.is_empty()).then_some(trimmed);
            let url = server_url.clone();
            in_flight.set(true);
            spawn(async move {
                match data::set_display_name(&url, value).await {
                    Ok(()) => {
                        msg.set(Some("Profile saved.".to_string()));
                        msg_is_error.set(false);
                        refresh_user();
                    }
                    Err(e) => {
                        msg.set(Some(e.to_string()));
                        msg_is_error.set(true);
                    }
                }
                in_flight.set(false);
            });
        }
    };

    let on_pick_avatar = {
        let server_url = server_url.clone();
        use_file_upload(
            uploading,
            upload_error,
            |_| None,
            move |filename, mime, bytes| {
                let url = server_url.clone();
                async move { data::upload_avatar(&url, filename, mime, bytes).await }
            },
            move |()| {
                // `Signal` is `Copy` and shares its state, so a local mutable
                // copy bumps the same counter from this `Fn` closure.
                let mut bust = bust;
                bust += 1;
                refresh_user();
            },
        )
    };

    let on_remove_avatar = {
        let server_url = server_url.clone();
        move |_| {
            let url = server_url.clone();
            uploading.set(true);
            spawn(async move {
                match data::delete_avatar(&url).await {
                    Ok(()) => {
                        upload_error.set(None);
                        bust += 1;
                        refresh_user();
                    }
                    Err(e) => upload_error.set(Some(format!("Remove failed: {e}"))),
                }
                uploading.set(false);
            });
        }
    };

    let has_avatar = user.as_ref().is_some_and(|u| u.has_avatar);
    let display = user
        .as_ref()
        .map(|u| u.display().to_string())
        .unwrap_or_default();

    rsx! {
        section { class: "card", "data-testid": "account-profile-card",
            h2 { "Account" }
            p { class: "subtitle", "How you appear across the library." }

            div { class: "profile-identity",
                div { class: "profile-avatar",
                    UserAvatar {
                        user_id: user.as_ref().map(|u| u.id).unwrap_or_default(),
                        name: display.clone(),
                        has_avatar,
                        class: "profile-initials",
                        bust: bust(),
                    }
                }
                div { class: "profile-avatar-actions",
                    div { class: "profile-avatar-buttons",
                        // A `<label>` rather than a button: it forwards the
                        // click to the hidden file input, which is the only
                        // element that can open the picker.
                        label {
                            class: "btn sm",
                            r#for: "avatar-file",
                            "Change picture"
                        }
                        input {
                            r#type: "file",
                            id: "avatar-file",
                            class: "profile-avatar-input",
                            "data-testid": "avatar-file-input",
                            accept: "image/jpeg,image/png,image/webp,image/gif",
                            disabled: uploading(),
                            onchange: on_pick_avatar,
                        }
                        if has_avatar {
                            button {
                                r#type: "button",
                                class: "btn ghost sm",
                                "data-testid": "avatar-remove",
                                disabled: uploading(),
                                onclick: on_remove_avatar,
                                "Remove"
                            }
                        }
                    }
                    p { class: "subtitle", "JPEG, PNG, WebP or GIF, up to 10 MB." }
                }
            }

            {credential_status_message("avatar-status", upload_error().as_deref(), true)}

            form {
                id: "profile-form",
                class: "settings-form",
                onsubmit: on_save,
                div { class: "settings-field",
                    label { r#for: "display-name", "Display name" }
                    input {
                        r#type: "text",
                        id: "display-name",
                        name: "display_name",
                        "data-testid": "display-name-input",
                        autocomplete: "nickname",
                        maxlength: "64",
                        placeholder: "{display}",
                        value: "{name_input}",
                        oninput: move |e| {
                            name_input.set(e.value());
                            msg.set(None);
                        },
                    }
                }
                p { class: "subtitle",
                    "Shown on your journals, ratings and shelves. Leave it empty to use your username."
                }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        disabled: in_flight(),
                        "data-testid": "display-name-save",
                        "Save"
                    }
                }
            }

            {credential_status_message("profile-status", msg().as_deref(), msg_is_error())}
        }
    }
}
