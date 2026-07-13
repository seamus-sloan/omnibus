//! Account screen (`/account`).
//!
//! Web/SSR renders the Send-to-Kindle destination form; the native
//! shell renders the mobile "You" tab (identity, now-reading, quick links,
//! account rows, theme). Signals start empty so SSR and the first WASM paint
//! agree (rule 07); the load effects fill them after mount.

use dioxus::prelude::*;

#[cfg(not(feature = "mobile"))]
use crate::{data, use_server_url};

#[cfg(feature = "mobile")]
use dioxus_router::{use_navigator, Link};
#[cfg(feature = "mobile")]
use omnibus_shared::{EbookMetadata, UserSummary};

#[cfg(feature = "mobile")]
use crate::components::atrium::{Cover, Theme};
#[cfg(feature = "mobile")]
use crate::{data, thumb_url, use_server_url, Route};

// ── Pure helpers (unit-tested) ─────────────────────────────────────

/// Two-letter uppercase initials for the avatar chip, derived from a
/// username. Splits on separators (space, `.`, `_`, `-`); a single-token
/// name uses its first two letters. Falls back to `"?"` when empty.
#[cfg(any(feature = "mobile", test))]
pub fn initials_for(username: &str) -> String {
    let tokens: Vec<&str> = username
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

/// Role label shown after the username in the identity subline. Admins are
/// "Owner" (matching the design mockup); everyone else is "Reader".
#[cfg(any(feature = "mobile", test))]
pub fn role_label(is_admin: bool) -> &'static str {
    if is_admin {
        "Owner"
    } else {
        "Reader"
    }
}

/// The identity subline: `username · role`.
#[cfg(any(feature = "mobile", test))]
pub fn identity_subline(username: &str, is_admin: bool) -> String {
    format!("{username} \u{00b7} {}", role_label(is_admin))
}

/// Cycle order for the theme segmented control.
#[cfg(any(feature = "mobile", test))]
pub const THEME_ORDER: [(&str, ThemeKind); 3] = [
    ("Dark", ThemeKind::Dark),
    ("Light", ThemeKind::Light),
    ("Sepia", ThemeKind::Sepia),
];

/// Target-agnostic mirror of [`crate::components::atrium::Theme`] used by the
/// pure helpers so the theme-selection logic is testable without the mobile
/// feature. Maps 1:1 to `Theme` on mobile via [`ThemeKind::to_theme`].
#[cfg(any(feature = "mobile", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
    Sepia,
}

#[cfg(any(feature = "mobile", test))]
impl ThemeKind {
    /// Convert to the atrium [`Theme`] applied to the signal.
    #[cfg(feature = "mobile")]
    fn to_theme(self) -> Theme {
        match self {
            ThemeKind::Dark => Theme::Dark,
            ThemeKind::Light => Theme::Light,
            ThemeKind::Sepia => Theme::Sepia,
        }
    }

    /// The `data-theme` attribute string this kind maps to. Lets the pure
    /// tests assert the segmented control lines up with the CSS themes
    /// without depending on the mobile-only `Theme`.
    pub fn as_attr(self) -> &'static str {
        match self {
            ThemeKind::Dark => "dark",
            ThemeKind::Light => "light",
            ThemeKind::Sepia => "sepia",
        }
    }
}

// ── Page ───────────────────────────────────────────────────────────

/// The Account screen. Web/SSR renders the Send-to-Kindle destination form;
/// the native shell renders the mobile "You" tab.
#[component]
pub fn AccountPage() -> Element {
    #[cfg(feature = "mobile")]
    {
        account_body()
    }
    #[cfg(not(feature = "mobile"))]
    {
        kindle_account_body()
    }
}

/// Web/SSR page body — the Send-to-Kindle destination form. Hydrates the
/// saved address from `/api/auth/me`; saving/clearing round-trips through
/// `data::set_kindle_email`.
#[cfg(not(feature = "mobile"))]
fn kindle_account_body() -> Element {
    let server_url = use_server_url();
    let mut email_input = use_signal(String::new);
    let mut saved_email = use_signal(|| None::<String>);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    // Hydrate the saved Kindle email after mount. `current_user` is a no-op on
    // SSR/mobile, so the first paint matches the empty-signal SSR markup.
    use_effect(move || {
        spawn(async move {
            if let Ok(Some(user)) = data::current_user().await {
                if let Some(email) = user.kindle_email.clone() {
                    email_input.set(email.clone());
                    saved_email.set(Some(email));
                }
            }
        });
    });

    let save_url = server_url.clone();
    let on_save = move |evt: Event<FormData>| {
        evt.prevent_default();
        let value = email_input().trim().to_string();
        if value.is_empty() {
            msg.set(Some("Enter a Kindle email to save.".to_string()));
            msg_is_error.set(true);
            return;
        }
        let url = save_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::set_kindle_email(&url, Some(value.clone())).await {
                Ok(()) => {
                    saved_email.set(Some(value));
                    msg.set(Some("Kindle email saved.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some(
                        "Failed to save Kindle email — check the address.".to_string(),
                    ));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let clear_url = server_url;
    let on_clear = move |_| {
        let url = clear_url.clone();
        in_flight.set(true);
        spawn(async move {
            match data::set_kindle_email(&url, None).await {
                Ok(()) => {
                    email_input.set(String::new());
                    saved_email.set(None);
                    msg.set(Some("Kindle email cleared.".to_string()));
                    msg_is_error.set(false);
                }
                Err(_) => {
                    msg.set(Some("Failed to clear Kindle email.".to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let connected = saved_email().is_some();

    rsx! {
        section { class: "card", "data-testid": "account-kindle-card",
            h1 { "Account" }
            p { class: "subtitle", "Configure your Send-to-Kindle delivery address." }

            form {
                id: "kindle-email-form",
                class: "settings-form",
                onsubmit: on_save,
                div { class: "settings-field",
                    label { r#for: "kindle-email", "Kindle Email" }
                    input {
                        r#type: "email",
                        id: "kindle-email",
                        name: "kindle_email",
                        "data-testid": "kindle-email-input",
                        autocomplete: "off",
                        autocapitalize: "none",
                        autocorrect: "off",
                        spellcheck: "false",
                        placeholder: "you@kindle.com",
                        value: "{email_input}",
                        oninput: move |e| email_input.set(e.value()),
                    }
                }
                p { class: "subtitle",
                    "Add "
                    b { "your library's sender address" }
                    " to your Amazon "
                    a {
                        href: "https://www.amazon.com/sendtokindle/email",
                        target: "_blank",
                        rel: "noopener",
                        "approved sender list"
                    }
                    " or Amazon will silently drop the delivery."
                }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        disabled: in_flight(),
                        "data-testid": "kindle-email-save",
                        "Save"
                    }
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: in_flight() || !connected,
                        "data-testid": "kindle-email-clear",
                        onclick: on_clear,
                        "Clear"
                    }
                }
            }

            p {
                class: if connected { "settings-status success" } else { "settings-status" },
                "data-testid": "kindle-email-connected",
                if connected { "A Kindle email is configured." } else { "No Kindle email configured yet." }
            }

            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "kindle-email-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

/// Mobile page body — owns the signals, effects, and rsx for the "You" tab.
#[cfg(feature = "mobile")]
fn account_body() -> Element {
    let server_url = use_server_url();
    let mut user = use_signal(|| None::<UserSummary>);
    let mut now_reading = use_signal(|| None::<EbookMetadata>);

    // Resolve the current user from the bearer token. A failure leaves the
    // signal `None` so the identity block keeps its placeholder rather than
    // flashing an error — a transient blip shouldn't blank the account tab.
    let me_url = server_url.clone();
    use_effect(move || {
        let url = me_url.clone();
        spawn(async move {
            if let Ok(u) = data::get_me(&url).await {
                user.set(Some(u));
            }
        });
    });

    // "Now reading" stand-in: there is no mobile in-progress endpoint yet, so
    // surface the first library book as the card's subject. The card hides
    // entirely when the library is empty or the fetch fails.
    let books_url = server_url.clone();
    use_effect(move || {
        let url = books_url.clone();
        spawn(async move {
            if let Ok(page) = data::get_ebooks(&url).await {
                now_reading.set(page.books.into_iter().next());
            }
        });
    });

    rsx! {
        div { class: "m-account", "data-testid": "account-screen",
            AccountIdentity { user: user() }
            NowReadingCard { book: now_reading(), server_url: server_url.clone() }
            QuickGrid {}
            AccountRows {}
            ThemeControl {}
        }
    }
}

/// Identity block: avatar chip + serif name + `username · role` subline.
/// Renders placeholders until the user resolves.
#[cfg(feature = "mobile")]
#[component]
fn AccountIdentity(user: Option<UserSummary>) -> Element {
    let (name, initials, subline) = match &user {
        Some(u) => (
            u.username.clone(),
            initials_for(&u.username),
            identity_subline(&u.username, u.is_admin),
        ),
        None => (
            "\u{2014}".to_string(),
            "\u{2026}".to_string(),
            String::new(),
        ),
    };
    rsx! {
        div { class: "m-account-identity",
            div { class: "m-account-avatar", "aria-hidden": "true", "{initials}" }
            div { class: "m-account-idcol",
                div { class: "m-account-name", "{name}" }
                if !subline.is_empty() {
                    div { class: "m-account-sub", "{subline}" }
                }
            }
        }
    }
}

/// "Now reading" card — cover + label + title + progress bar, tinted by the
/// book's accent. Hidden when no book is available.
#[cfg(feature = "mobile")]
#[component]
fn NowReadingCard(book: Option<EbookMetadata>, server_url: String) -> Element {
    let Some(b) = book else {
        return rsx! {};
    };
    let accent_style = b
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    let title = b
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| b.filename.clone());
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    // Only override with the thumb endpoint when we have a real uuid — an empty
    // one builds `/api/thumbs//sm` and hides the valid `cover_url` fallback.
    let cover_src = b
        .cover_url
        .as_ref()
        .filter(|_| !uuid.is_empty())
        .map(|_| thumb_url(&server_url, &uuid, "sm"));

    rsx! {
        div { class: "card m-account-nowreading", style: "{accent_style}",
            div { class: "m-account-nr-cover",
                Cover { book: b.clone(), src_override: cover_src, sizes: Some("46px".to_string()) }
            }
            div { class: "m-account-nr-body",
                div { class: "label", "Now reading" }
                div { class: "m-account-nr-title", "{title}" }
                div { class: "pbar", i {} }
            }
        }
    }
}

/// 2×2 quick-links grid. Shelves links to the real shelves route; the rest
/// are static labels until their backing surfaces exist on mobile.
#[cfg(feature = "mobile")]
#[component]
fn QuickGrid() -> Element {
    rsx! {
        div { class: "m-account-grid",
            div { class: "card m-account-tile",
                div { class: "m-account-tile-label", "Journal" }
                div { class: "m-account-tile-sub", "Reading notes" }
            }
            div { class: "card m-account-tile",
                div { class: "m-account-tile-label", "Highlights" }
                div { class: "m-account-tile-sub", "Saved passages" }
            }
            Link { to: Route::Shelves {}, class: "card m-account-tile",
                div { class: "m-account-tile-label", "Shelves" }
                div { class: "m-account-tile-sub", "Your collections" }
            }
            div { class: "card m-account-tile",
                div { class: "m-account-tile-label", "Goals" }
                div { class: "m-account-tile-sub", "Reading targets" }
            }
        }
    }
}

/// Account list rows: Settings, Admin · server, Add books, Sign out. The
/// first three navigate to existing routes; "Sign out" clears the token and
/// returns to the login screen.
#[cfg(feature = "mobile")]
#[component]
fn AccountRows() -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();

    let on_sign_out = move |_| {
        let url = server_url.clone();
        spawn(async move {
            let _ = data::mobile_logout(&url).await;
            nav.replace(Route::Login {});
        });
    };

    rsx! {
        div { class: "m-account-rows",
            AccountLinkRow { to: Route::Settings {}, label: "Settings" }
            // No dedicated admin route exists; the Settings page hosts the
            // admin-only library-path controls, so route there.
            AccountLinkRow { to: Route::Settings {}, label: "Admin \u{00b7} server" }
            AccountLinkRow { to: Route::AddBooks {}, label: "Add books" }
            button {
                r#type: "button",
                class: "m-account-row m-account-row-danger",
                "data-testid": "account-sign-out",
                onclick: on_sign_out,
                "Sign out"
            }
        }
    }
}

/// One navigable list row with a trailing chevron.
#[cfg(feature = "mobile")]
#[component]
fn AccountLinkRow(to: Route, label: String) -> Element {
    rsx! {
        Link { to, class: "m-account-row",
            span { "{label}" }
            span { class: "m-account-chevron", "aria-hidden": "true", "\u{203a}" }
        }
    }
}

/// Dark / Light / Sepia segmented control wired to the app-wide theme signal.
#[cfg(feature = "mobile")]
#[component]
fn ThemeControl() -> Element {
    let mut theme = use_context::<Signal<Theme>>();
    let current = theme.read().as_attr();
    rsx! {
        div { class: "m-account-theme",
            span { class: "label", "Theme" }
            div { class: "m-account-seg", role: "group", "aria-label": "Theme",
                for (name , kind) in THEME_ORDER {
                    button {
                        key: "{name}",
                        r#type: "button",
                        class: if kind.as_attr() == current { "m-account-seg-btn on" } else { "m-account-seg-btn" },
                        "aria-pressed": if kind.as_attr() == current { "true" } else { "false" },
                        onclick: move |_| {
                            theme.set(kind.to_theme());
                            crate::components::atrium::persist_theme(kind.to_theme());
                        },
                        "{name}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_uses_first_letters_of_two_tokens() {
        assert_eq!(initials_for("elena koval"), "EK");
        assert_eq!(initials_for("ada.lovelace"), "AL");
        assert_eq!(initials_for("grace_hopper"), "GH");
    }

    #[test]
    fn initials_takes_two_letters_of_single_token() {
        assert_eq!(initials_for("seamus"), "SE");
        assert_eq!(initials_for("a"), "A");
    }

    #[test]
    fn initials_falls_back_for_empty_or_separator_only() {
        assert_eq!(initials_for(""), "?");
        assert_eq!(initials_for("   "), "?");
        assert_eq!(initials_for("..._"), "?");
    }

    #[test]
    fn role_label_maps_admin_to_owner() {
        assert_eq!(role_label(true), "Owner");
        assert_eq!(role_label(false), "Reader");
    }

    #[test]
    fn identity_subline_joins_username_and_role() {
        assert_eq!(identity_subline("elena", true), "elena \u{00b7} Owner");
        assert_eq!(identity_subline("bob", false), "bob \u{00b7} Reader");
    }

    #[test]
    fn theme_order_matches_css_attrs() {
        let attrs: Vec<&str> = THEME_ORDER.iter().map(|(_, k)| k.as_attr()).collect();
        assert_eq!(attrs, ["dark", "light", "sepia"]);
    }
}
