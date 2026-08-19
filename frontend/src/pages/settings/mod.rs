//! Settings page (`/settings`) — a permission-gated sidebar over configuration
//! sections, with the active section driven by the `?section=` query param.
//! Non-admins see the per-user sections (Account, Kindle, Kobo); admins also see
//! Library Location, Metadata Lookup, Email delivery, Users, and Logs. The
//! mobile shell has no sidebar — it renders those cards flat.

// Web/server only: the Background Tasks dashboard calls the web-only
// `data::get_background_tasks` (no mobile RPC route yet), same shape as
// `health`'s "Last errors" panel.
#[cfg(not(feature = "mobile"))]
mod background_tasks;
// Web/server only: the Library cleanup section calls the web-only
// `data::get_cleanup_counts`/`run_cleanup_detection` (no mobile surface).
#[cfg(not(feature = "mobile"))]
mod cleanup;
// Web/server only: the format-conversion card calls the web-only
// `data::get_ebook_convert`/`save_ebook_convert` (no mobile RPC route yet).
#[cfg(not(feature = "mobile"))]
mod ebook_convert;
// Web/server only: the Server Health "Last errors" panel calls the web-only
// `data::get_last_errors` (no mobile RPC route yet), same shape as `logs`.
#[cfg(not(feature = "mobile"))]
mod health;
mod library;
// Only compiled on web/server: its body calls the web-only
// `data::get_metadata_precedence`/`save_metadata_precedence` (no mobile RPC
// route yet).
#[cfg(not(feature = "mobile"))]
mod metadata_precedence;
mod secret_key_field;
mod smtp;
// Web/server only: the admin Users surface calls the web-only `data::users`
// REST wrappers (no mobile admin surface).
#[cfg(not(feature = "mobile"))]
mod users;

use dioxus::prelude::*;

#[cfg(not(feature = "mobile"))]
use background_tasks::BackgroundTasksSection;
#[cfg(not(feature = "mobile"))]
use cleanup::CleanupSection;
#[cfg(not(feature = "mobile"))]
use ebook_convert::EbookConvertField;
#[cfg(not(feature = "mobile"))]
use health::LastErrorsSection;
use library::LibraryLocationSection;
use secret_key_field::{SecretKeyField, SecretKeyKind};
use smtp::SmtpConfigField;
#[cfg(not(feature = "mobile"))]
use users::UsersSection;

/// The `/settings` page. Web/server render the sectioned sidebar; the native
/// shell renders the flat mobile form (`section` is web-only navigation state).
#[cfg(not(feature = "mobile"))]
#[component]
pub fn SettingsPage(section: Option<String>) -> Element {
    // Admin gating is derived from the app-wide `CurrentUser` context
    // (`use_is_admin`); the server-side `AdminUser` extractor on the config
    // RPCs is the real boundary. This just keeps admin sections off a
    // non-admin screen. Starts `false` on SSR/first paint (context unresolved)
    // so hydration matches, then reconciles after the boot effect (rule 07).
    let is_admin = crate::use_is_admin();
    let active = parse_section(section.as_deref(), is_admin());

    rsx! {
        div { class: "settings-layout", "data-testid": "settings-layout",
            SettingsSidebar { active, is_admin: is_admin() }
            div { class: "settings-content",
                {section_content(active)}
            }
        }
    }
}

/// Mobile settings: the library form plus the admin key/SMTP cards, flat (no
/// sidebar, no metadata-precedence or logs surfaces — those stay web-only).
#[cfg(feature = "mobile")]
#[component]
pub fn SettingsPage(section: Option<String>) -> Element {
    let _ = section;
    let is_admin = crate::use_is_admin();
    rsx! {
        div { class: "settings-page",
            LibraryLocationSection {}
            if is_admin() {
                SecretKeyField { kind: SecretKeyKind::Hardcover }
                SecretKeyField { kind: SecretKeyKind::GoogleBooks }
                SmtpConfigField {}
            }
        }
    }
}

/// A configuration section reachable from the sidebar. `Account`, `Kindle`,
/// and `Kobo` are per-user and visible to everyone; every other section
/// configures the server and is admin-only.
#[cfg(not(feature = "mobile"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Account,
    Kindle,
    Kobo,
    Library,
    Metadata,
    Email,
    Users,
    Logs,
    Health,
    BackgroundTasks,
    Cleanup,
}

#[cfg(not(feature = "mobile"))]
impl SettingsSection {
    /// The `?section=` slug — the deep-link identity and active-item key.
    fn slug(self) -> &'static str {
        match self {
            SettingsSection::Account => "account",
            SettingsSection::Kindle => "kindle",
            SettingsSection::Kobo => "kobo",
            SettingsSection::Library => "library",
            SettingsSection::Metadata => "metadata",
            SettingsSection::Email => "email",
            SettingsSection::Users => "users",
            SettingsSection::Logs => "logs",
            SettingsSection::Health => "health",
            SettingsSection::BackgroundTasks => "background-tasks",
            SettingsSection::Cleanup => "cleanup",
        }
    }

    /// Whether the section is hidden from non-admins.
    fn requires_admin(self) -> bool {
        !matches!(
            self,
            SettingsSection::Account | SettingsSection::Kindle | SettingsSection::Kobo
        )
    }
}

/// Resolve the `?section=` slug to a section, falling back to Account for an
/// unknown slug or when a non-admin requests an admin-only section.
#[cfg(not(feature = "mobile"))]
fn parse_section(raw: Option<&str>, is_admin: bool) -> SettingsSection {
    let requested = match raw {
        Some("kindle") => SettingsSection::Kindle,
        Some("kobo") => SettingsSection::Kobo,
        Some("library") => SettingsSection::Library,
        Some("metadata") => SettingsSection::Metadata,
        Some("email") => SettingsSection::Email,
        Some("users") => SettingsSection::Users,
        Some("logs") => SettingsSection::Logs,
        Some("health") => SettingsSection::Health,
        Some("background-tasks") => SettingsSection::BackgroundTasks,
        Some("cleanup") => SettingsSection::Cleanup,
        _ => SettingsSection::Account,
    };
    if requested.requires_admin() && !is_admin {
        SettingsSection::Account
    } else {
        requested
    }
}

/// The card(s) for the active section. Each reuses the existing controls
/// unchanged — the reorg only moves them behind the sidebar.
#[cfg(not(feature = "mobile"))]
fn section_content(active: SettingsSection) -> Element {
    match active {
        SettingsSection::Account => rsx! { super::AccountPage {} },
        SettingsSection::Kindle => rsx! { super::account::KindleEmailCard {} },
        SettingsSection::Kobo => rsx! { super::account::kobo::KoboDevicesCard {} },
        SettingsSection::Library => rsx! {
            LibraryLocationSection {}
            EbookConvertField {}
        },
        SettingsSection::Metadata => rsx! {
            metadata_precedence::MetadataPrecedenceField {}
            SecretKeyField { kind: SecretKeyKind::Hardcover }
            SecretKeyField { kind: SecretKeyKind::GoogleBooks }
        },
        SettingsSection::Email => rsx! { SmtpConfigField {} },
        SettingsSection::Users => rsx! { UsersSection {} },
        SettingsSection::Logs => rsx! { super::LogsPage {} },
        SettingsSection::Health => rsx! { LastErrorsSection {} },
        SettingsSection::BackgroundTasks => rsx! { BackgroundTasksSection {} },
        SettingsSection::Cleanup => rsx! { CleanupSection {} },
    }
}

/// Left sidebar: title, permission-gated nav groups, and (for non-admins) a
/// note that server settings are admin-managed.
#[cfg(not(feature = "mobile"))]
#[component]
fn SettingsSidebar(active: SettingsSection, is_admin: bool) -> Element {
    let subtitle = if is_admin {
        "Server administration"
    } else {
        "Your account"
    };
    rsx! {
        nav { class: "settings-sidebar", "aria-label": "Settings sections",
            div { class: "settings-sidebar-head",
                h1 { "Settings" }
                p { class: "subtitle", "{subtitle}" }
            }

            SettingsNavGroup { label: "Account",
                SettingsNavItem { section: SettingsSection::Account, active, label: "Account", icon: "◉" }
                SettingsNavItem { section: SettingsSection::Kindle, active, label: "Kindle", icon: "▤" }
                SettingsNavItem { section: SettingsSection::Kobo, active, label: "Kobo", icon: "▦" }
            }

            if is_admin {
                SettingsNavGroup { label: "Library",
                    SettingsNavItem { section: SettingsSection::Library, active, label: "Library Location", icon: "≡" }
                    SettingsNavItem { section: SettingsSection::Metadata, active, label: "Metadata Lookup", icon: "◆" }
                    SettingsNavItem { section: SettingsSection::Email, active, label: "Email delivery", icon: "✉" }
                    SettingsNavItem { section: SettingsSection::Cleanup, active, label: "Library cleanup", icon: "✦" }
                }
                SettingsNavGroup { label: "Administration",
                    SettingsNavItem { section: SettingsSection::Users, active, label: "Users", icon: "☰" }
                    SettingsNavItem { section: SettingsSection::Logs, active, label: "Logs", icon: "▣" }
                    SettingsNavItem { section: SettingsSection::Health, active, label: "Server Health", icon: "♥" }
                    SettingsNavItem { section: SettingsSection::BackgroundTasks, active, label: "Background Tasks", icon: "⏱" }
                }
            } else {
                p { class: "settings-sidebar-note",
                    "Server settings are managed by an administrator."
                }
            }
        }
    }
}

/// A labelled group of sidebar items (e.g. "ACCOUNT").
#[cfg(not(feature = "mobile"))]
#[component]
fn SettingsNavGroup(label: String, children: Element) -> Element {
    rsx! {
        div { class: "settings-nav-group",
            div { class: "settings-nav-group-label", "{label}" }
            {children}
        }
    }
}

/// A single sidebar entry: an icon + label link to `/settings?section=<slug>`,
/// marked active (and `aria-current`) when it is the resolved section.
#[cfg(not(feature = "mobile"))]
#[component]
fn SettingsNavItem(
    section: SettingsSection,
    active: SettingsSection,
    label: String,
    icon: String,
) -> Element {
    use dioxus_router::Link;

    use crate::Route;

    let is_active = section == active;
    let class = if is_active {
        "settings-nav-item active"
    } else {
        "settings-nav-item"
    };
    rsx! {
        Link {
            to: Route::Settings { section: Some(section.slug().to_string()) },
            class: "{class}",
            "data-testid": "settings-nav-{section.slug()}",
            "aria-current": if is_active { "page" } else { "false" },
            span { class: "settings-nav-icon", "aria-hidden": "true", "{icon}" }
            span { class: "settings-nav-label", "{label}" }
        }
    }
}

#[cfg(all(test, not(feature = "mobile")))]
mod tests {
    use super::*;

    #[test]
    fn parse_section_defaults_to_account_when_unset() {
        assert!(matches!(
            parse_section(None, true),
            SettingsSection::Account
        ));
        assert!(matches!(
            parse_section(None, false),
            SettingsSection::Account
        ));
    }

    #[test]
    fn parse_section_maps_known_slugs_for_admin() {
        assert!(matches!(
            parse_section(Some("library"), true),
            SettingsSection::Library
        ));
        assert!(matches!(
            parse_section(Some("logs"), true),
            SettingsSection::Logs
        ));
        assert!(matches!(
            parse_section(Some("health"), true),
            SettingsSection::Health
        ));
        assert!(matches!(
            parse_section(Some("background-tasks"), true),
            SettingsSection::BackgroundTasks
        ));
        assert!(matches!(
            parse_section(Some("cleanup"), true),
            SettingsSection::Cleanup
        ));
    }

    #[test]
    fn parse_section_maps_per_user_slugs_for_everyone() {
        assert!(matches!(
            parse_section(Some("kindle"), false),
            SettingsSection::Kindle
        ));
        assert!(matches!(
            parse_section(Some("kobo"), false),
            SettingsSection::Kobo
        ));
        assert!(matches!(
            parse_section(Some("kindle"), true),
            SettingsSection::Kindle
        ));
        assert!(matches!(
            parse_section(Some("kobo"), true),
            SettingsSection::Kobo
        ));
    }

    #[test]
    fn parse_section_falls_back_to_account_for_non_admin_admin_section() {
        assert!(matches!(
            parse_section(Some("logs"), false),
            SettingsSection::Account
        ));
        assert!(matches!(
            parse_section(Some("metadata"), false),
            SettingsSection::Account
        ));
        assert!(matches!(
            parse_section(Some("health"), false),
            SettingsSection::Account
        ));
        assert!(matches!(
            parse_section(Some("background-tasks"), false),
            SettingsSection::Account
        ));
        assert!(matches!(
            parse_section(Some("cleanup"), false),
            SettingsSection::Account
        ));
    }

    #[test]
    fn parse_section_falls_back_to_account_for_unknown_slug() {
        assert!(matches!(
            parse_section(Some("nope"), true),
            SettingsSection::Account
        ));
    }
}
