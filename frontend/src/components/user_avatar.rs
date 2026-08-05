//! The one place a user is drawn as a circle: their uploaded avatar, or a
//! monogram when they have none.
//!
//! Every monogram site (user menu trigger and panel, journal bylines, ratings
//! rows, the mobile "You" tab) mounts this so the photo/letter decision and the
//! broken-image fallback are written once. Callers pass the CSS class their
//! site already used, so this changes what is inside the circle, not its shape.

use dioxus::prelude::*;

/// Two-letter uppercase initials for a monogram, derived from a display name.
/// Splits on separators (space, `.`, `_`, `-`); a single-token name uses its
/// first two letters. Falls back to `"?"` when empty.
pub fn initials_for(name: &str) -> String {
    let tokens: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || matches!(c, '.' | '_' | '-'))
        .filter(|t| !t.is_empty())
        .collect();
    let picked: String = match tokens.as_slice() {
        [] => return "?".to_string(),
        [single] => single.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    picked.to_uppercase()
}

/// A user's avatar image, or their monogram when they haven't set one.
///
/// `class` is the caller's existing site class (`um-initials`,
/// `bd-journal-avatar`, …); the image variant adds `<class>--photo` so each
/// site can size its own circle. `bust` comes from
/// [`crate::contexts::AvatarCacheBust`] — the URL is keyed only by user id, so
/// a replaced avatar produces the same URL and needs the counter to reload.
///
/// SSR and the first WASM paint agree because the markup is a pure function of
/// the props (rule 07): before the user resolves, callers pass `has_avatar:
/// false` and get the same monogram on both sides.
#[component]
pub fn UserAvatar(
    user_id: i64,
    name: String,
    has_avatar: bool,
    class: String,
    #[props(default)] bust: u32,
) -> Element {
    // Track the *url* that failed rather than a bool, so a mobile token
    // rotation or a cache-bust bump gets a fresh attempt instead of being
    // stuck on the monogram until a full reload.
    let mut broken_src: Signal<Option<String>> = use_signal(|| None);
    let server_url = crate::use_server_url();
    let url = has_avatar.then(|| {
        crate::contexts::append_cache_bust(
            crate::media_url(&server_url, &format!("/api/users/{user_id}/avatar")),
            bust,
        )
    });
    let photo_class = format!("{class}-photo");
    let shown = url.filter(|u| broken_src.read().as_deref() != Some(u.as_str()));
    let is_monogram = shown.is_none();

    // The outer span is the same element in both states, so swapping a photo
    // in or out only ever diffs its children — never replaces the node its
    // siblings' event handlers were registered alongside.
    rsx! {
        span {
            class: "{class}",
            "aria-hidden": if is_monogram { "true" },
            if let Some(url) = shown {
                img {
                    class: "{photo_class}",
                    src: "{url}",
                    alt: "{name}",
                    onerror: {
                        let url = url.clone();
                        move |_| broken_src.set(Some(url.clone()))
                    },
                }
            } else {
                "{initials_for(&name)}"
            }
        }
    }
}

#[cfg(test)]
mod tests;
