//! Profile card at the top of Settings → Account: display name + avatar.
//! Both writes are account configuration, so they go straight to the server
//! and surface failures inline (rule 08). Signals start empty so SSR and the
//! first WASM paint agree (rule 07).

use dioxus::prelude::*;
use omnibus_shared::UserSummary;

use crate::components::credential_card::credential_status_message;
use crate::components::image_upload::use_file_upload;
use crate::components::user_avatar::UserAvatar;
use crate::{data, use_server_url};

/// Signals backing the profile card — grouped so the seeding effect and the
/// three write-handler builders don't each take seven signal params. Mirrors
/// `KindleEmailSignals` in `account.rs`.
#[derive(Clone, Copy)]
struct ProfileSignals {
    name_input: Signal<String>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    in_flight: Signal<bool>,
    uploading: Signal<bool>,
    upload_error: Signal<Option<String>>,
    // Seeded once from the resolved user; a later refresh must not clobber
    // what the user is mid-way through typing.
    seeded: Signal<bool>,
}

/// Seeds `name_input` from the resolved `CurrentUser` once. Mirrors
/// `use_kindle_email_hydration`'s shape in `account.rs`.
fn use_profile_seed_hydration(
    current_user: Signal<Option<Option<UserSummary>>>,
    mut signals: ProfileSignals,
) {
    use_effect(move || {
        if (signals.seeded)() {
            return;
        }
        if let Some(u) = current_user().flatten() {
            signals
                .name_input
                .set(u.display_name.clone().unwrap_or_default());
            signals.seeded.set(true);
        }
    });
}

/// Re-reads `/api/auth/me` into the app-wide `CurrentUser` context, so the
/// user menu and journal bylines repaint without a reload rather than each
/// guessing at the new value.
fn refresh_current_user(mut current_user: Signal<Option<Option<UserSummary>>>) {
    spawn(async move {
        if let Ok(u) = data::current_user().await {
            current_user.set(Some(u));
        }
    });
}

/// Submit handler for the display-name save form. Mirrors
/// `kindle_save_handler`'s shape in `account.rs`.
fn profile_save_handler(
    server_url: String,
    current_user: Signal<Option<Option<UserSummary>>>,
    mut signals: ProfileSignals,
) -> impl FnMut(Event<FormData>) + 'static {
    move |evt: Event<FormData>| {
        evt.prevent_default();
        let trimmed = (signals.name_input)().trim().to_string();
        let value = (!trimmed.is_empty()).then_some(trimmed);
        let url = server_url.clone();
        signals.in_flight.set(true);
        spawn(async move {
            match data::set_display_name(&url, value).await {
                Ok(()) => {
                    signals.msg.set(Some("Profile saved.".to_string()));
                    signals.msg_is_error.set(false);
                    refresh_current_user(current_user);
                }
                Err(e) => {
                    signals.msg.set(Some(e.to_string()));
                    signals.msg_is_error.set(true);
                }
            }
            signals.in_flight.set(false);
        });
    }
}

/// `onchange` handler for the avatar file picker.
fn profile_avatar_upload_handler(
    server_url: String,
    current_user: Signal<Option<Option<UserSummary>>>,
    bust: Signal<u32>,
    signals: ProfileSignals,
) -> impl FnMut(Event<FormData>) + 'static {
    use_file_upload(
        signals.uploading,
        signals.upload_error,
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
            refresh_current_user(current_user);
        },
    )
}

/// Click handler for the avatar remove button.
fn profile_avatar_remove_handler(
    server_url: String,
    current_user: Signal<Option<Option<UserSummary>>>,
    bust: Signal<u32>,
    mut signals: ProfileSignals,
) -> impl FnMut(Event<MouseData>) + 'static {
    move |_| {
        let url = server_url.clone();
        let mut bust = bust;
        signals.uploading.set(true);
        spawn(async move {
            match data::delete_avatar(&url).await {
                Ok(()) => {
                    signals.upload_error.set(None);
                    bust += 1;
                    refresh_current_user(current_user);
                }
                Err(e) => signals
                    .upload_error
                    .set(Some(format!("Remove failed: {e}"))),
            }
            signals.uploading.set(false);
        });
    }
}

/// Read-only display values for `profile_avatar_section`, grouped so the
/// function stays under clippy's `too_many_arguments` cap.
struct ProfileAvatarView<'a> {
    user_id: i64,
    display: &'a str,
    has_avatar: bool,
    bust: u32,
    uploading: bool,
    upload_error: Option<&'a str>,
}

/// The avatar identity + change/remove controls and their status line. Split
/// out of `ProfileCard` so the handler wiring above and this markup each
/// stay readable on their own — mirrors `kindle_email_form`'s split in
/// `account.rs`.
fn profile_avatar_section(
    view: ProfileAvatarView<'_>,
    on_pick_avatar: impl FnMut(Event<FormData>) + 'static,
    on_remove_avatar: impl FnMut(Event<MouseData>) + 'static,
) -> Element {
    let ProfileAvatarView {
        user_id,
        display,
        has_avatar,
        bust,
        uploading,
        upload_error,
    } = view;
    rsx! {
        div { class: "profile-identity",
            div { class: "profile-avatar",
                UserAvatar {
                    user_id,
                    name: display.to_string(),
                    has_avatar,
                    class: "profile-initials",
                    bust,
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
                        disabled: uploading,
                        onchange: on_pick_avatar,
                    }
                    if has_avatar {
                        button {
                            r#type: "button",
                            class: "btn ghost sm",
                            "data-testid": "avatar-remove",
                            disabled: uploading,
                            onclick: on_remove_avatar,
                            "Remove"
                        }
                    }
                }
                p { class: "subtitle", "JPEG, PNG, WebP or GIF, up to 10 MB." }
            }
        }

        {credential_status_message("avatar-status", upload_error, true)}
    }
}

/// The display-name form: input, save action, and the helper hint. Mirrors
/// `kindle_email_form`'s split in `account.rs`.
fn profile_name_form(
    mut name_input: Signal<String>,
    mut msg: Signal<Option<String>>,
    in_flight: bool,
    display: &str,
    on_save: impl FnMut(Event<FormData>) + 'static,
) -> Element {
    rsx! {
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
                    disabled: in_flight,
                    "data-testid": "display-name-save",
                    "Save"
                }
            }
        }
    }
}

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
    let bust = crate::use_avatar_cache_bust().0;

    let signals = ProfileSignals {
        name_input: use_signal(String::new),
        msg: use_signal(|| None::<String>),
        msg_is_error: use_signal(|| false),
        in_flight: use_signal(|| false),
        uploading: use_signal(|| false),
        upload_error: use_signal(|| None::<String>),
        seeded: use_signal(|| false),
    };
    use_profile_seed_hydration(current_user, signals);

    let user = current_user().flatten();
    let on_save = profile_save_handler(server_url.clone(), current_user, signals);
    let on_pick_avatar =
        profile_avatar_upload_handler(server_url.clone(), current_user, bust, signals);
    let on_remove_avatar = profile_avatar_remove_handler(server_url, current_user, bust, signals);

    let has_avatar = user.as_ref().is_some_and(|u| u.has_avatar);
    let display = user
        .as_ref()
        .map(|u| u.display().to_string())
        .unwrap_or_default();

    rsx! {
        section { class: "card", "data-testid": "account-profile-card",
            h2 { "Account" }
            p { class: "subtitle", "How you appear across the library." }

            {profile_avatar_section(
                ProfileAvatarView {
                    user_id: user.as_ref().map(|u| u.id).unwrap_or_default(),
                    display: &display,
                    has_avatar,
                    bust: bust(),
                    uploading: (signals.uploading)(),
                    upload_error: (signals.upload_error)().as_deref(),
                },
                on_pick_avatar,
                on_remove_avatar,
            )}

            {profile_name_form(signals.name_input, signals.msg, (signals.in_flight)(), &display, on_save)}

            {credential_status_message("profile-status", (signals.msg)().as_deref(), (signals.msg_is_error)())}
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
