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
