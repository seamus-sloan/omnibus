//! User-menu dropdown mounted in the top nav. Real wiring: Settings,
//! Sign out, Dark/Light theme. Everything else is a stubbed `<a>`.
//!
//! SSR/hydration: the trigger is always rendered (empty placeholder
//! initials in the pre-hydration phase) so the topbar markup is stable
//! and `expectNavVisible` finds it without racing the auth ping. The
//! App-wide [`crate::CurrentUser`] context — populated once by the boot
//! effect in [`crate::App`] — then either fills in real initials
//! (auth'd) or swaps the trigger for a `Log in` link (unauth). The panel
//! only opens once we have a real user.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::UserSummary;

use crate::components::atrium::{persist_theme, Theme};
use crate::{use_current_user, Route};

/// Derive 1–2 character avatar initials from a username. Empty input falls
/// back to "?".
pub(crate) fn initials_for(username: &str) -> String {
    let trimmed = username.trim();
    if trimmed.is_empty() {
        return "?".into();
    }
    trimmed.chars().take(2).collect::<String>().to_uppercase()
}

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
        let initials = user
            .as_ref()
            .map(|u| initials_for(&u.username))
            .unwrap_or_default();
        rsx! {
            div { class: "um-root",
                UserMenuTrigger { initials, open }
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
fn UserMenuTrigger(initials: String, open: Signal<bool>) -> Element {
    let mut open = open;
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
            span { class: "um-initials", "{initials}" }
        }
    }
}

#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn UserMenuPanel(user: UserSummary, open: Signal<bool>) -> Element {
    let mut open = open;
    let nav = use_navigator();
    let theme = use_context::<Signal<Theme>>();

    let role = if user.is_admin { "Owner" } else { "Member" };
    let handle = format!("{}@local", user.username);

    let on_signout = move |_| {
        // Spawn first; closing the menu unmounts the button mid-handler on
        // web, which can swallow work queued after `open.set(false)`.
        //
        // Hydration parity (rule 07): the closure body must be identical
        // across SSR/WASM. `data::logout` has a non-web `Ok(())` stub so
        // this dispatches uniformly — SSR never fires click handlers, and
        // WASM runs the real REST call.
        spawn(async move {
            let _ = crate::data::logout().await;
            open.set(false);
            nav.replace(Route::Login {});
        });
    };

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
                // across SSR/WASM; `focus_user_menu_panel` has a non-web
                // no-op stub so the gate lives at the function definition,
                // not inside the rsx-attached handler.
                focus_user_menu_panel(&evt);
            },

            // ── Header ────────────────────────────────────────────
            div { class: "um-header",
                div { class: "um-avatar um-avatar-lg",
                    span { class: "um-initials", "{initials_for(&user.username)}" }
                }
                div { class: "um-identity",
                    div { class: "um-name", "{user.username}" }
                    div { class: "um-handle",
                        "{handle} · "
                        span { class: "um-role", "{role}" }
                    }
                }
                a {
                    class: "um-edit",
                    href: "#",
                    "aria-disabled": "true",
                    tabindex: "-1",
                    onclick: move |evt| evt.prevent_default(),
                    "Edit"
                }
            }

            // ── Now reading (stub) ───────────────────────────────
            div { class: "um-section",
                div { class: "um-section-label", "NOW READING" }
                a {
                    class: "um-now-reading",
                    href: "#",
                    "aria-disabled": "true",
                    tabindex: "-1",
                    onclick: move |evt| evt.prevent_default(),
                    div { class: "um-nr-cover" }
                    div { class: "um-nr-meta",
                        div { class: "um-nr-title", "Piranesi" }
                        div { class: "um-nr-author", "Susanna Clarke" }
                        div { class: "um-nr-progress",
                            div { class: "um-nr-pbar", div { class: "um-nr-pbar-fill" } }
                            div { class: "um-nr-stats",
                                span { "68%" }
                                span { "ch. 22" }
                                span { "4h 12m left" }
                            }
                        }
                    }
                }
            }

            // ── Stat grid (stubs) ────────────────────────────────
            div { class: "um-stat-grid",
                UmStat { label: "Journal", detail: "24 entries" }
                UmStat { label: "Highlights", detail: "412 quotes" }
                UmStat { label: "Shelves", detail: "3 shared" }
                UmStat { label: "Goals", detail: "12 / 24 books" }
            }

            // ── Linear rows (account) ────────────────────────────
            div { class: "um-rows",
                Link {
                    to: Route::Settings {},
                    class: "um-row",
                    onclick: move |_| open.set(false),
                    span { class: "um-row-icon", "⚙" }
                    span { class: "um-row-label", "Settings" }
                }
                a {
                    class: "um-row",
                    href: "#",
                    "aria-disabled": "true",
                    tabindex: "-1",
                    onclick: move |evt| evt.prevent_default(),
                    span { class: "um-row-icon", "▣" }
                    span { class: "um-row-label", "Admin · server health" }
                    span { class: "um-row-aside", "all ok" }
                }
                a {
                    class: "um-row",
                    href: "#",
                    "aria-disabled": "true",
                    tabindex: "-1",
                    onclick: move |evt| evt.prevent_default(),
                    span { class: "um-row-icon", "◔" }
                    span { class: "um-row-label", "Notifications" }
                    span { class: "um-row-badge", "2" }
                }
            }

            // ── Linear rows (session) ────────────────────────────
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
                    onclick: on_signout,
                    span { class: "um-row-icon", "⏻" }
                    span { class: "um-row-label", "Sign out" }
                }
            }

            // ── Theme footer ─────────────────────────────────────
            UmThemeSeg { theme }
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

/// Focus the user-menu panel after the browser has painted it so the
/// `onkeydown` handler attached to it actually receives ESC. Same
/// requestAnimationFrame timing pattern as
/// `search_palette::focus_palette_input`: calling `.focus()` synchronously
/// inside `onmounted` lands before layout completes and the focus call
/// no-ops.
#[cfg(feature = "web")]
fn focus_user_menu_panel(evt: &MountedEvent) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::prelude::*;

    let Some(element) = evt.try_as_web_event() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::once_into_js(move || {
        if let Some(html_el) = element.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

/// Non-web stub: SSR never paints the panel and native shells don't
/// drive the user menu, so there is nothing to focus. Defined so the
/// `onmounted` handler can call `focus_user_menu_panel` unconditionally
/// (rule 07: hydration parity — keep cfg gates out of rsx bodies).
#[cfg(not(feature = "web"))]
fn focus_user_menu_panel(_evt: &MountedEvent) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_uppercases_first_two_chars() {
        assert_eq!(initials_for("seamus"), "SE");
        assert_eq!(initials_for("ek"), "EK");
    }

    #[test]
    fn initials_handles_short_input() {
        assert_eq!(initials_for("a"), "A");
    }

    #[test]
    fn initials_fallback_for_empty() {
        assert_eq!(initials_for(""), "?");
        assert_eq!(initials_for("   "), "?");
    }
}
