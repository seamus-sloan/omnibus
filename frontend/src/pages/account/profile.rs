//! Profile card at the top of Settings → Account: display name + avatar.
//! Both writes are account configuration, so they go straight to the server
//! and surface failures inline (rule 08). Signals start empty so SSR and the
//! first WASM paint agree (rule 07).

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

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(all(test, feature = "server"))]
mod tests {
    use omnibus_shared::UserSummary;

    use super::*;
    use crate::test_support::{render, render_in_vdom};
    use crate::{AvatarCacheBust, CurrentUser};

    fn resolved_user(display_name: Option<&str>, has_avatar: bool) -> UserSummary {
        UserSummary {
            id: 42,
            username: "reader".to_string(),
            is_admin: false,
            can_upload: false,
            can_edit: false,
            can_download: false,
            kindle_email: None,
            display_name: display_name.map(str::to_string),
            has_avatar,
            hidden_formats: Vec::new(),
        }
    }

    /// Card mounted before `CurrentUser` has resolved — SSR and the very
    /// first WASM paint, before the boot effect lands.
    fn card_unresolved() -> Element {
        use_context_provider(|| CurrentUser(Signal::new(None)));
        use_context_provider(|| AvatarCacheBust(Signal::new(0u32)));
        rsx! {
            ProfileCard {}
        }
    }

    /// A resolved user with an avatar, seen through a non-zero
    /// `AvatarCacheBust` counter (as if a prior upload this session already
    /// bumped it).
    fn card_with_avatar() -> Element {
        use_context_provider(|| {
            CurrentUser(Signal::new(Some(Some(resolved_user(
                Some("Jane Doe"),
                true,
            )))))
        });
        use_context_provider(|| AvatarCacheBust(Signal::new(3u32)));
        rsx! {
            ProfileCard {}
        }
    }

    /// A resolved user with no avatar uploaded.
    fn card_without_avatar() -> Element {
        use_context_provider(|| CurrentUser(Signal::new(Some(Some(resolved_user(None, false))))));
        use_context_provider(|| AvatarCacheBust(Signal::new(0u32)));
        rsx! {
            ProfileCard {}
        }
    }

    #[test]
    fn profile_card_renders_the_unresolved_steady_state_before_current_user_resolves() {
        let html = render_in_vdom(card_unresolved);
        assert!(html.contains("data-testid=\"account-profile-card\""));
        assert!(html.contains("data-testid=\"display-name-input\""));
        assert!(html.contains("value=\"\""));
        // Nothing to remove and nothing to report until a resolved user says
        // otherwise.
        assert!(!html.contains("data-testid=\"avatar-remove\""));
        assert!(!html.contains("data-testid=\"avatar-status\""));
        assert!(!html.contains("data-testid=\"profile-status\""));
    }

    /// The `seeded` guard means `name_input` stays empty on first paint even
    /// with a resolved user in context — seeding is a deliberate post-mount
    /// effect, never folded into the initial render (keeps SSR/first-paint
    /// in agreement per rule 07).
    #[test]
    fn profile_card_leaves_the_name_input_unseeded_on_first_paint_even_with_a_resolved_user() {
        let html = render_in_vdom(card_with_avatar);
        assert!(html.contains("data-testid=\"display-name-input\""));
        assert!(html.contains("value=\"\""));
        // The placeholder is a pure function of the resolved user, so it
        // shows through immediately — only the editable value waits on the
        // seeding effect.
        assert!(html.contains("placeholder=\"Jane Doe\""));
    }

    /// A resolved user with `has_avatar: true` gets the Remove action, and
    /// the avatar `<img>` URL carries the `AvatarCacheBust` counter — the
    /// mechanism that makes a same-URL avatar replacement actually re-fetch
    /// (`on_pick_avatar`/`on_remove_avatar` both bump it on success).
    #[test]
    fn profile_card_shows_remove_and_busts_the_avatar_url_when_the_user_has_one() {
        let html = render_in_vdom(card_with_avatar);
        assert!(html.contains("data-testid=\"avatar-remove\""));
        assert!(html.contains("/api/users/42/avatar?v=3"));
    }

    /// No avatar on the resolved user means nothing to remove, and
    /// `UserAvatar` never renders an image URL at all (falls back to the
    /// monogram) — so there is nothing for the `bust` counter to affect.
    #[test]
    fn profile_card_hides_remove_and_the_avatar_url_when_the_user_has_none() {
        let html = render_in_vdom(card_without_avatar);
        assert!(!html.contains("data-testid=\"avatar-remove\""));
        assert!(!html.contains("/api/users/42/avatar"));
    }

    /// `on_remove_avatar`'s error arm writes `"Remove failed: {e}"` into the
    /// same `avatar-status` slot rendered here via `credential_status_message`.
    /// Unlike the upload-failure path, no Playwright spec drives a remove
    /// failure end-to-end, so this is the only coverage of this branch.
    #[test]
    fn avatar_status_slot_renders_a_remove_failure_with_error_styling() {
        let html = render(credential_status_message(
            "avatar-status",
            Some("Remove failed: connection refused"),
            true,
        ));
        assert!(html.contains("data-testid=\"avatar-status\""));
        assert!(html.contains("settings-status error"));
        assert!(html.contains("Remove failed: connection refused"));
    }

    /// `use_file_upload`'s error arm writes `"Upload failed: {e}"` into the
    /// same slot for a failed avatar pick.
    #[test]
    fn avatar_status_slot_renders_an_upload_failure_with_error_styling() {
        let html = render(credential_status_message(
            "avatar-status",
            Some("Upload failed: 413 Payload Too Large"),
            true,
        ));
        assert!(html.contains("settings-status error"));
        assert!(html.contains("Upload failed: 413 Payload Too Large"));
    }

    /// The confirmation `on_save` writes into `profile-status` on `Ok(())`.
    #[test]
    fn profile_status_slot_renders_the_save_confirmation_without_error_styling() {
        let html = render(credential_status_message(
            "profile-status",
            Some("Profile saved."),
            false,
        ));
        assert!(html.contains("data-testid=\"profile-status\""));
        assert!(html.contains("settings-status success"));
        assert!(!html.contains("settings-status error"));
    }
}
