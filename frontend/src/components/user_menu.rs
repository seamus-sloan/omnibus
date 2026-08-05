//! User-menu dropdown mounted in the top nav. Real wiring: recent progress,
//! Settings, Sign out, Dark/Light theme, and app version. Other account
//! surfaces remain stubbed. See [`UserMenu`] for the SSR/hydration handling
//! of the pre-auth trigger state.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{ProgressFormat, ResumePoint, UserSummary};

use crate::components::atrium::{persist_theme, Cover, Theme};
use crate::components::glyphs::{book_glyph, play_glyph};
use crate::components::user_avatar::UserAvatar;
use crate::focus_after_paint::focus_after_paint;
use crate::{use_current_user, Route};

/// Renders the user menu component (web and server targets).
///
/// SSR/hydration: the trigger is always rendered (empty placeholder
/// initials in the pre-hydration phase) so the topbar markup is stable and
/// `expectNavVisible` finds it without racing the auth ping. The App-wide
/// [`crate::CurrentUser`] context — populated once by the boot effect in
/// [`crate::App`] — then either fills in real initials (auth'd) or swaps
/// the trigger for a `Log in` link (unauth). The panel only opens once we
/// have a real user.
#[cfg(any(feature = "web", feature = "server"))]
#[component]
pub fn UserMenu() -> Element {
    let mut open = use_signal(|| false);

    // Read the App-wide cached `/api/auth/me` result instead of firing our
    // own fetch on every mount. The boot effect in `App` fills this in
    // once; we just re-render reactively when it changes.
    //
    // Outer `None` = not yet resolved (pre-hydration on SSR, or a
    // transient error left the cache empty). Inner `None` = explicit 401.
    // Inner `Some(u)` = authenticated.
    let snapshot = use_current_user().0();
    // SSR / pre-hydration: render the trigger (empty placeholder initials)
    // so the topbar markup is stable. WASM hydrates the same DOM, then the
    // App-level boot effect resolves the cached user: if Some(user),
    // initials fill in and the dropdown becomes interactive; if None,
    // swap to a Log-in link. The panel only renders when we have a real
    // user — clicking the placeholder trigger before auth resolves is a
    // no-op visually.
    let user = match &snapshot {
        Some(Some(u)) => Some(u.clone()),
        _ => None,
    };
    let unauth = matches!(snapshot, Some(None));

    if unauth {
        rsx! {
            Link {
                to: Route::Login {},
                class: "btn ghost sm",
                "Log in"
            }
        }
    } else {
        rsx! {
            div { class: "um-root",
                UserMenuTrigger { user: user.clone(), open }
                if open() {
                    if let Some(user) = user {
                        div {
                            class: "um-scrim",
                            "data-testid": "user-menu-scrim",
                            onclick: move |_| open.set(false),
                        }
                        UserMenuPanel { user, open }
                    }
                }
            }
        }
    }
}

#[cfg(not(any(feature = "web", feature = "server")))]
#[component]
pub fn UserMenu() -> Element {
    rsx! {}
}

#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UserMenuTrigger(user: Option<UserSummary>, open: Signal<bool>) -> Element {
    let mut open = open;
    let bust = crate::use_avatar_cache_bust().0;
    rsx! {
        button {
            class: "um-trigger",
            "data-testid": "user-menu-trigger",
            "aria-label": "Open user menu",
            "aria-haspopup": "dialog",
            "aria-expanded": "{open()}",
            r#type: "button",
            onclick: move |_| {
                let next = !open();
                open.set(next);
            },
            // Before the boot effect resolves the user (SSR and the first
            // WASM paint alike) this is an empty monogram, so both renders
            // agree and hydration adopts cleanly (rule 07).
            if let Some(u) = user {
                UserAvatar {
                    user_id: u.id,
                    name: u.display().to_string(),
                    has_avatar: u.has_avatar,
                    class: "um-initials",
                    bust: bust(),
                }
            } else {
                span { class: "um-initials" }
            }
        }
    }
}

#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UserMenuPanel(user: UserSummary, open: Signal<bool>) -> Element {
    let mut open = open;
    let nav = use_navigator();
    let theme = use_context::<Signal<Theme>>();
    let on_signout = build_on_signout(open, nav);

    let on_keydown = move |evt: Event<KeyboardData>| {
        if evt.key() == Key::Escape {
            evt.prevent_default();
            open.set(false);
        }
    };

    rsx! {
        div {
            class: "um-panel",
            "data-testid": "user-menu-panel",
            role: "dialog",
            "aria-label": "User menu",
            tabindex: "-1",
            onkeydown: on_keydown,
            onmounted: move |evt: MountedEvent| {
                // Focus the panel so the onkeydown listener above receives
                // ESC presses — opening the menu via the trigger leaves
                // focus on the (now-unmounted-from-DOM-tab-order) trigger
                // otherwise. Defer to the next animation frame so the
                // browser has finished layout before `.focus()` lands.
                //
                // Hydration parity (rule 07): closure body stays identical
                // across SSR/WASM; `focus_after_paint` has a non-web no-op
                // stub so the gate lives at the function definition, not
                // inside the rsx-attached handler.
                focus_after_paint(&evt);
            },

            UmHeader { user, open }
            UmNowReading {}

            div { class: "um-stat-grid",
                UmStat { label: "Journal", detail: "24 entries" }
                UmStat { label: "Highlights", detail: "412 quotes" }
                UmStat { label: "Shelves", detail: "3 shared" }
                UmStat { label: "Goals", detail: "12 / 24 books" }
            }

            UmAccountRows { open }
            UmSessionRows { on_signout }

            UmThemeSeg { theme }
            UmVersion {}
        }
    }
}

/// Sign-out action: logs out, closes the menu, then routes to the login
/// page. Spawn-first ordering matters — closing the menu unmounts the
/// button mid-handler on web, which can swallow work queued after
/// `open.set(false)`.
///
/// Hydration parity (rule 07): the closure body must be identical across
/// SSR/WASM. `data::logout` has a non-web `Ok(())` stub so this
/// dispatches uniformly — SSR never fires click handlers, and WASM runs
/// the real REST call.
fn build_on_signout(mut open: Signal<bool>, nav: dioxus_router::Navigator) -> EventHandler<()> {
    EventHandler::new(move |()| {
        spawn(async move {
            let _ = crate::data::logout().await;
            open.set(false);
            nav.replace(Route::Login {});
        });
    })
}

/// Avatar + display-name/handle/role identity block, plus the "Edit" link
/// into the Account settings section.
///
/// The handle stays the *username*: it is the stable login identity, and a
/// display name can change or collide.
#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmHeader(user: UserSummary, open: Signal<bool>) -> Element {
    let mut open = open;
    let role = if user.is_admin { "Owner" } else { "Member" };
    let handle = format!("{}@local", user.username);
    let bust = crate::use_avatar_cache_bust().0;
    rsx! {
        div { class: "um-header",
            div { class: "um-avatar um-avatar-lg",
                UserAvatar {
                    user_id: user.id,
                    name: user.display().to_string(),
                    has_avatar: user.has_avatar,
                    class: "um-initials",
                    bust: bust(),
                }
            }
            div { class: "um-identity",
                div { class: "um-name", "{user.display()}" }
                div { class: "um-handle",
                    "{handle} · "
                    span { class: "um-role", "{role}" }
                }
            }
            Link {
                class: "um-edit",
                to: Route::Settings { section: Some("account".to_string()) },
                onclick: move |_| open.set(false),
                "Edit"
            }
        }
    }
}

/// Most recently progressed book across reading and listening.
#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmNowReading() -> Element {
    let mut recent = use_signal(|| None::<Result<Option<ResumePoint>, String>>);

    use_effect(move || {
        spawn(async move {
            let result = crate::data::recent_progress("", 1)
                .await
                .map(|points| points.into_iter().next())
                .map_err(|error| error.to_string());
            recent.set(Some(result));
        });
    });

    rsx! {
        div { class: "um-section",
            div { class: "um-section-label", "NOW READING" }
            match recent() {
                None => rsx! {
                    div { class: "um-now-reading um-now-reading-state", role: "status",
                        "Loading reading progress..."
                    }
                },
                Some(Ok(None)) => rsx! {
                    div { class: "um-now-reading um-now-reading-state",
                        "Nothing in progress"
                    }
                },
                Some(Err(_)) => rsx! {
                    div { class: "um-now-reading um-now-reading-state error", role: "alert",
                        "Unable to load reading progress."
                    }
                },
                Some(Ok(Some(point))) => um_now_reading_row(point),
            }
        }
    }
}

/// The "in progress" row for [`UmNowReading`]: cover, title, author, and a
/// resume link that routes to the reader or the player depending on format.
#[cfg(any(feature = "web", feature = "server"))]
fn um_now_reading_row(point: ResumePoint) -> Element {
    let is_audio = point.record.format == ProgressFormat::Audio;
    let title = point
        .book
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&point.book.filename)
        .to_string();
    let author = point
        .book
        .creators
        .first()
        .map(|creator| creator.name.clone())
        .filter(|author| !author.trim().is_empty());
    let action = if is_audio {
        "Continue listening"
    } else {
        "Continue reading"
    };
    let uuid = point.record.book_uuid.clone();
    let to = crate::routes::resume_route(&point);
    let detail = Route::BookDetail { uuid };
    let action_label = format!("{action} {title}");

    rsx! {
        div {
            class: "um-now-reading",
            "data-testid": "user-menu-now-reading",
            Link {
                to: detail.clone(),
                class: "um-nr-cover",
                "aria-label": "View details for {title}",
                Cover { book: point.book }
            }
            div { class: "um-nr-meta",
                Link {
                    to: detail,
                    class: "um-nr-info",
                    div { class: "um-nr-title", "{title}" }
                    if let Some(author) = author {
                        div { class: "um-nr-author", "{author}" }
                    }
                }
                div { class: "um-nr-action", "{action}" }
            }
            Link {
                to,
                class: "um-nr-play",
                "data-testid": "user-menu-now-reading-action",
                "aria-label": "{action_label}",
                if is_audio {
                    {play_glyph(14)}
                } else {
                    {book_glyph(14)}
                }
            }
        }
    }
}

/// Account-scoped linear rows. Everything an account needs now lives under
/// Settings (Account, server config, and Logs are all sections there), so this
/// is a single Settings row that closes the menu on click.
#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmAccountRows(open: Signal<bool>) -> Element {
    let mut open = open;
    rsx! {
        div { class: "um-rows",
            Link {
                to: Route::Settings { section: None },
                class: "um-row",
                onclick: move |_| open.set(false),
                span { class: "um-row-icon", "⚙" }
                span { class: "um-row-label", "Settings" }
            }
        }
    }
}

/// Session-scoped linear rows: the stubbed "Switch user" row and the
/// real Sign-out button.
#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmSessionRows(on_signout: EventHandler<()>) -> Element {
    rsx! {
        div { class: "um-rows",
            a {
                class: "um-row",
                href: "#",
                "aria-disabled": "true",
                tabindex: "-1",
                onclick: move |evt| evt.prevent_default(),
                span { class: "um-row-icon", "⇄" }
                span { class: "um-row-label", "Switch user" }
            }
            button {
                class: "um-row destructive",
                "data-testid": "logout-button",
                r#type: "button",
                onclick: move |_| on_signout.call(()),
                span { class: "um-row-icon", "⏻" }
                span { class: "um-row-label", "Sign out" }
            }
        }
    }
}

#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmStat(label: String, detail: String) -> Element {
    rsx! {
        a {
            class: "um-stat",
            href: "#",
            "aria-disabled": "true",
            tabindex: "-1",
            onclick: move |evt| evt.prevent_default(),
            div { class: "um-stat-label", "{label}" }
            div { class: "um-stat-detail", "{detail}" }
        }
    }
}

#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmThemeSeg(theme: Signal<Theme>) -> Element {
    let mut theme = theme;
    let current = *theme.read();
    let dark_on = matches!(current, Theme::Dark);
    let black_on = matches!(current, Theme::Black);
    let light_on = matches!(current, Theme::Light);
    let sepia_on = matches!(current, Theme::Sepia);
    rsx! {
        div { class: "um-theme",
            div { class: "um-section-label", "THEME" }
            div { class: "um-theme-seg", role: "group", "aria-label": "Theme",
                button {
                    class: if dark_on { "um-theme-btn on" } else { "um-theme-btn" },
                    r#type: "button",
                    "data-testid": "theme-dark",
                    onclick: move |_| {
                        theme.set(Theme::Dark);
                        persist_theme(Theme::Dark);
                    },
                    "Dark"
                }
                button {
                    class: if black_on { "um-theme-btn on" } else { "um-theme-btn" },
                    r#type: "button",
                    "data-testid": "theme-black",
                    onclick: move |_| {
                        theme.set(Theme::Black);
                        persist_theme(Theme::Black);
                    },
                    "Black"
                }
                button {
                    class: if light_on { "um-theme-btn on" } else { "um-theme-btn" },
                    r#type: "button",
                    "data-testid": "theme-light",
                    onclick: move |_| {
                        theme.set(Theme::Light);
                        persist_theme(Theme::Light);
                    },
                    "Light"
                }
                button {
                    class: if sepia_on { "um-theme-btn on" } else { "um-theme-btn" },
                    r#type: "button",
                    "data-testid": "theme-sepia",
                    onclick: move |_| {
                        theme.set(Theme::Sepia);
                        persist_theme(Theme::Sepia);
                    },
                    "Sepia"
                }
            }
        }
    }
}

/// Single version line at the bottom of the panel. The web app is served by
/// the server, so the two are always the same build — render the
/// compile-time constant directly rather than fetching `/api/_health` (AC1).
/// A compile-time constant renders identically on SSR and first WASM paint,
/// so there's no hydration-parity concern here (rule 07).
#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UmVersion() -> Element {
    rsx! {
        div { class: "um-version", "data-testid": "user-menu-version",
            "{crate::version::app_version()}"
        }
    }
}
