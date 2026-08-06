//! Account screen (`/account`).
//!
//! Web/SSR renders the self-service account cards (password today); the
//! Kindle delivery address and Kobo devices live in their own Settings
//! sections, built from [`KindleEmailCard`] and [`kobo::KoboDevicesCard`].
//! The native shell renders the mobile "You" tab (identity, now-reading,
//! quick links, account rows, theme, server/app version). Signals start
//! empty so SSR and the first WASM paint agree (rule 07); the load effects
//! fill them after mount.

use dioxus::prelude::*;

#[cfg(feature = "mobile")]
mod downloads;
// Web-only Kobo wireless-sync device card, rendered as its own Settings
// section.
#[cfg(not(feature = "mobile"))]
pub(crate) mod kobo;
// Display name + avatar, at the top of the Account section.
#[cfg(not(feature = "mobile"))]
mod profile;

#[cfg(not(feature = "mobile"))]
use crate::components::auth::{score_password, PasswordRequirements, StrengthMeter};
#[cfg(not(feature = "mobile"))]
use crate::components::credential_card::credential_status_message;
#[cfg(not(feature = "mobile"))]
use crate::{data, use_server_url};

#[cfg(feature = "mobile")]
use dioxus_router::{use_navigator, Link};
#[cfg(feature = "mobile")]
use omnibus_shared::{EbookMetadata, UserSummary};

#[cfg(feature = "mobile")]
use crate::components::atrium::{Cover, Theme};
#[cfg(feature = "mobile")]
use crate::components::user_avatar::UserAvatar;
#[cfg(feature = "mobile")]
use crate::pages::CheckInOpen;
#[cfg(feature = "mobile")]
use crate::{data, thumb_url, use_server_url, Route};

// ── Pure helpers (unit-tested) ─────────────────────────────────────

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
pub const THEME_ORDER: [(&str, ThemeKind); 4] = [
    ("Dark", ThemeKind::Dark),
    ("Black", ThemeKind::Black),
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
    Black,
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
            ThemeKind::Black => Theme::Black,
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
            ThemeKind::Black => "black",
            ThemeKind::Light => "light",
            ThemeKind::Sepia => "sepia",
        }
    }
}

// ── Page ───────────────────────────────────────────────────────────

/// The Account screen. Web/SSR renders the self-service account cards; the
/// native shell renders the mobile "You" tab.
#[component]
pub fn AccountPage() -> Element {
    #[cfg(feature = "mobile")]
    {
        account_body()
    }
    #[cfg(not(feature = "mobile"))]
    {
        account_web_body()
    }
}

/// Signals backing the Kindle email form — grouped so the hydration effect
/// and the save/clear handler builders don't each take five signal params.
#[cfg(not(feature = "mobile"))]
#[derive(Clone, Copy)]
struct KindleEmailSignals {
    email_input: Signal<String>,
    saved_email: Signal<Option<String>>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    in_flight: Signal<bool>,
}

/// Hydrates the saved Kindle email after mount. `current_user` is a no-op on
/// SSR/mobile, so the first paint matches the empty-signal SSR markup.
#[cfg(not(feature = "mobile"))]
fn use_kindle_email_hydration(mut signals: KindleEmailSignals) {
    use_effect(move || {
        spawn(async move {
            if let Ok(Some(user)) = data::current_user().await {
                if let Some(email) = user.kindle_email.clone() {
                    signals.email_input.set(email.clone());
                    signals.saved_email.set(Some(email));
                }
            }
        });
    });
}

/// Submit handler for the Kindle email save form: validates non-empty
/// locally, then round-trips through `data::set_kindle_email`.
#[cfg(not(feature = "mobile"))]
fn kindle_save_handler(
    server_url: String,
    mut signals: KindleEmailSignals,
) -> impl FnMut(Event<FormData>) + 'static {
    move |evt: Event<FormData>| {
        evt.prevent_default();
        let value = (signals.email_input)().trim().to_string();
        if value.is_empty() {
            signals
                .msg
                .set(Some("Enter a Kindle email to save.".to_string()));
            signals.msg_is_error.set(true);
            return;
        }
        let url = server_url.clone();
        signals.in_flight.set(true);
        spawn(async move {
            match data::set_kindle_email(&url, Some(value.clone())).await {
                Ok(()) => {
                    signals.saved_email.set(Some(value));
                    signals.msg.set(Some("Kindle email saved.".to_string()));
                    signals.msg_is_error.set(false);
                }
                Err(_) => {
                    signals.msg.set(Some(
                        "Failed to save Kindle email — check the address.".to_string(),
                    ));
                    signals.msg_is_error.set(true);
                }
            }
            signals.in_flight.set(false);
        });
    }
}

/// Click handler for the Kindle email clear button.
#[cfg(not(feature = "mobile"))]
fn kindle_clear_handler(
    server_url: String,
    mut signals: KindleEmailSignals,
) -> impl FnMut(Event<MouseData>) + 'static {
    move |_| {
        let url = server_url.clone();
        signals.in_flight.set(true);
        spawn(async move {
            match data::set_kindle_email(&url, None).await {
                Ok(()) => {
                    signals.email_input.set(String::new());
                    signals.saved_email.set(None);
                    signals.msg.set(Some("Kindle email cleared.".to_string()));
                    signals.msg_is_error.set(false);
                }
                Err(_) => {
                    signals
                        .msg
                        .set(Some("Failed to clear Kindle email.".to_string()));
                    signals.msg_is_error.set(true);
                }
            }
            signals.in_flight.set(false);
        });
    }
}

/// The Kindle email form: input, save/clear actions, and the approved-sender
/// hint. Split out of `kindle_account_body` so the signal wiring above and
/// this markup each stay readable on their own.
#[cfg(not(feature = "mobile"))]
fn kindle_email_form(
    email_input: Signal<String>,
    in_flight: Signal<bool>,
    connected: bool,
    on_save: impl FnMut(Event<FormData>) + 'static,
    on_clear: impl FnMut(Event<MouseData>) + 'static,
) -> Element {
    let mut email_input = email_input;
    rsx! {
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
    }
}

/// Web/SSR Account section body — the self-service cards that aren't tied to
/// a delivery device. Kindle and Kobo have their own Settings sections.
#[cfg(not(feature = "mobile"))]
fn account_web_body() -> Element {
    rsx! {
        profile::ProfileCard {}
        ChangePasswordCard {}
    }
}

/// The Send-to-Kindle destination card (Settings → Kindle). Hydrates the
/// saved address from `/api/auth/me`; saving/clearing round-trips through
/// `data::set_kindle_email`.
#[cfg(not(feature = "mobile"))]
#[component]
pub(crate) fn KindleEmailCard() -> Element {
    let server_url = use_server_url();
    let signals = KindleEmailSignals {
        email_input: use_signal(String::new),
        saved_email: use_signal(|| None::<String>),
        msg: use_signal(|| None::<String>),
        msg_is_error: use_signal(|| false),
        in_flight: use_signal(|| false),
    };
    use_kindle_email_hydration(signals);

    let on_save = kindle_save_handler(server_url.clone(), signals);
    let on_clear = kindle_clear_handler(server_url, signals);
    let connected = (signals.saved_email)().is_some();

    rsx! {
        section { class: "card", "data-testid": "account-kindle-card",
            h2 { "Kindle" }
            p { class: "subtitle", "Configure your Send-to-Kindle delivery address." }

            {kindle_email_form(signals.email_input, signals.in_flight, connected, on_save, on_clear)}

            p {
                class: if connected { "settings-status success" } else { "settings-status" },
                "data-testid": "kindle-email-connected",
                if connected { "A Kindle email is configured." } else { "No Kindle email configured yet." }
            }

            {credential_status_message("kindle-email-status", (signals.msg)().as_deref(), (signals.msg_is_error)())}
        }
    }
}

/// Signals owned by [`ChangePasswordCard`].
#[cfg(not(feature = "mobile"))]
#[derive(Clone, Copy)]
struct ChangePasswordSignals {
    current: Signal<String>,
    new_password: Signal<String>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    in_flight: Signal<bool>,
}

/// Submit handler for the change-password form: validates both fields are
/// non-empty locally, then round-trips through `data::change_password`.
/// Mirrors `kindle_save_handler`'s shape above.
#[cfg(not(feature = "mobile"))]
fn change_password_submit_handler(
    server_url: String,
    mut signals: ChangePasswordSignals,
) -> impl FnMut(Event<FormData>) + 'static {
    move |evt: Event<FormData>| {
        evt.prevent_default();
        let cur = (signals.current)();
        let next = (signals.new_password)();
        if cur.is_empty() || next.is_empty() {
            signals
                .msg
                .set(Some("Enter your current and new password.".to_string()));
            signals.msg_is_error.set(true);
            return;
        }
        let url = server_url.clone();
        signals.in_flight.set(true);
        spawn(async move {
            match data::change_password(&url, cur, next).await {
                Ok(()) => {
                    signals.current.set(String::new());
                    signals.new_password.set(String::new());
                    signals.msg.set(Some("Password changed.".to_string()));
                    signals.msg_is_error.set(false);
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

/// Self-service change-password card (web Account section). Requires the
/// current password to authorize, validates the new one against the same
/// policy as registration (strength meter + requirements are advisory; the
/// server is authoritative), and surfaces the server's inline error for a
/// wrong current password or a policy rejection without clearing the form.
#[cfg(not(feature = "mobile"))]
#[component]
fn ChangePasswordCard() -> Element {
    let server_url = use_server_url();
    let signals = ChangePasswordSignals {
        current: use_signal(String::new),
        new_password: use_signal(String::new),
        msg: use_signal(|| None::<String>),
        msg_is_error: use_signal(|| false),
        in_flight: use_signal(|| false),
    };
    let mut current = signals.current;
    let mut new_password = signals.new_password;
    let mut msg = signals.msg;
    let msg_is_error = signals.msg_is_error;
    let in_flight = signals.in_flight;

    let pw = new_password();
    let (score, score_label, rules) = score_password(&pw);

    let on_submit = change_password_submit_handler(server_url, signals);

    rsx! {
        section { class: "card", "data-testid": "account-password-card",
            h2 { "Password" }
            p { class: "subtitle", "Change the password you use to sign in." }

            form {
                id: "change-password-form",
                class: "settings-form",
                onsubmit: on_submit,
                div { class: "settings-field",
                    label { r#for: "current-password", "Current password" }
                    input {
                        r#type: "password",
                        id: "current-password",
                        name: "current_password",
                        "data-testid": "current-password-input",
                        autocomplete: "current-password",
                        autocapitalize: "none",
                        autocorrect: "off",
                        spellcheck: "false",
                        value: "{current}",
                        oninput: move |e| {
                            current.set(e.value());
                            msg.set(None);
                        },
                    }
                }
                div { class: "settings-field",
                    label { r#for: "new-password", "New password" }
                    input {
                        r#type: "password",
                        id: "new-password",
                        name: "new_password",
                        "data-testid": "new-password-input",
                        autocomplete: "new-password",
                        autocapitalize: "none",
                        autocorrect: "off",
                        spellcheck: "false",
                        value: "{new_password}",
                        oninput: move |e| {
                            new_password.set(e.value());
                            msg.set(None);
                        },
                    }
                }
                StrengthMeter { score, label: Some(score_label.to_string()) }
                PasswordRequirements { rules }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        disabled: in_flight(),
                        "data-testid": "change-password-submit",
                        "Change password"
                    }
                }
            }

            {credential_status_message("change-password-status", msg().as_deref(), msg_is_error())}
        }
    }
}

/// Mobile page body — owns the signals, effects, and rsx for the "You" tab.
#[cfg(feature = "mobile")]
fn account_body() -> Element {
    let server_url = use_server_url();
    let mut user = use_signal(|| None::<UserSummary>);
    let mut now_reading = use_signal(|| None::<EbookMetadata>);
    // Seeded `None` so SSR and the first WASM paint agree (rule 07); the
    // effect below fills it in after mount.
    let mut server_version = use_signal(|| None::<String>);

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

    // Server release version, for comparison against the app's own build
    // (#1055). No mismatch/update prompt is shown — not every server
    // release has a matching mobile release, so a version gap doesn't
    // reliably mean an update exists. Users read and compare the two
    // numbers themselves.
    let version_url = server_url.clone();
    use_effect(move || {
        let url = version_url.clone();
        spawn(async move {
            if let Ok(v) = data::get_server_version(&url).await {
                server_version.set(Some(v));
            }
        });
    });

    rsx! {
        div { class: "m-account", "data-testid": "account-screen",
            AccountIdentity { user: user() }
            NowReadingCard { book: now_reading(), server_url: server_url.clone() }
            QuickGrid {}
            downloads::DownloadsSection {}
            AccountRows {}
            ThemeControl {}
            downloads::SyncStatusRow {}
            VersionBlock { server_version: server_version() }
        }
    }
}

/// Identity block: avatar chip + serif name + `username · role` subline.
/// Renders placeholders until the user resolves.
#[cfg(feature = "mobile")]
#[component]
fn AccountIdentity(user: Option<UserSummary>) -> Element {
    // The subline keeps the *username*: it's the stable login identity, and
    // the name above it may now be a display name.
    let (name, subline) = match &user {
        Some(u) => (
            u.display().to_string(),
            identity_subline(&u.username, u.is_admin),
        ),
        None => ("\u{2014}".to_string(), String::new()),
    };
    rsx! {
        div { class: "m-account-identity",
            div { class: "m-account-avatar",
                if let Some(u) = user.as_ref() {
                    UserAvatar {
                        user_id: u.id,
                        name: u.display().to_string(),
                        has_avatar: u.has_avatar,
                        class: "m-account-initials",
                    }
                } else {
                    span { class: "m-account-initials", "aria-hidden": "true", "\u{2026}" }
                }
            }
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

/// Account list rows: Settings, Admin · server (admins only), Add books,
/// Sign out. The link rows navigate to existing routes; "Sign out" clears
/// the token and returns to the login screen.
#[cfg(feature = "mobile")]
#[component]
fn AccountRows() -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();
    let is_admin = crate::use_is_admin();
    let mut check_in_open = use_context::<CheckInOpen>().0;

    let on_sign_out = move |_| {
        let url = server_url.clone();
        spawn(async move {
            let _ = data::mobile_logout(&url).await;
            nav.replace(Route::Login {});
        });
    };

    rsx! {
        div { class: "m-account-rows",
            AccountLinkRow { to: Route::Settings { section: None }, label: "Settings" }
            if is_admin() {
                // Mobile's SettingsPage ignores `section` (it renders one flat
                // form, gated by is_admin internally), so this lands on the
                // same page as "Settings" above — the row exists to give
                // admins a labeled shortcut, not a distinct destination, so
                // it's hidden from non-admins who'd otherwise land somewhere
                // identical to "Settings" under a misleading label.
                AccountLinkRow { to: Route::Settings { section: Some("library".into()) }, label: "Admin \u{00b7} server" }
            }
            AccountLinkRow { to: Route::AddBooks {}, label: "Add books" }
            // Check-in is a centered overlay, not a route — a button that
            // raises it, styled like the navigable rows around it.
            button {
                r#type: "button",
                class: "m-account-row",
                "data-testid": "account-check-in",
                onclick: move |_| check_in_open.set(true),
                span { "Check in a book" }
                span { class: "m-account-chevron", "aria-hidden": "true", "\u{203a}" }
            }
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

/// Dark / Black / Light / Sepia segmented control wired to the app-wide theme signal.
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

/// Server + app version lines at the bottom of the "You" tab (AC2): the
/// server's release (fetched from `/api/_health`, showing a placeholder
/// until the post-mount fetch in `account_body` resolves) above the app's
/// own compile-time build version. Deliberately no "update available"
/// prompt — see the effect comment in `account_body` for why a version gap
/// doesn't reliably indicate a pending update.
#[cfg(feature = "mobile")]
#[component]
fn VersionBlock(server_version: Option<String>) -> Element {
    let server = server_version.as_deref().unwrap_or("\u{2026}");
    rsx! {
        div { class: "m-account-version", "data-testid": "account-version",
            div { "Server version: {server}" }
            div { "App version: {crate::version::app_version()}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(attrs, ["dark", "black", "light", "sepia"]);
    }
}
