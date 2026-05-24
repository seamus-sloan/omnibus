use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route, Link};

use crate::components::atrium::ThemeToggle;
use crate::components::search_palette::SearchPaletteHost;
use crate::Route;

#[component]
pub fn TopNav() -> Element {
    let route = use_route::<Route>();
    // Hide the search trigger on `/settings` — the page has its own dense
    // form layout and a search button wedged into the nav above it just
    // clutters the chrome.
    let on_settings = matches!(route, Route::Settings {});
    let is_library = matches!(route, Route::Landing {});
    let is_authors = matches!(route, Route::AuthorsIndex {} | Route::AuthorDetail { .. });
    let is_series = matches!(route, Route::SeriesIndex {} | Route::SeriesDetail { .. });

    rsx! {
        nav { class: "atrium-topbar", aria_label: "Primary",
            Link {
                to: Route::Landing {},
                class: "atrium-brand",
                div { class: "atrium-brand-mark" }
                div { class: "atrium-brand-word", "Omnibus" }
            }
            div { class: "atrium-nav",
                Link {
                    to: Route::Landing {},
                    class: if is_library { "on" } else { "" },
                    "Library"
                }
                Link {
                    to: Route::AuthorsIndex {},
                    class: if is_authors { "on" } else { "" },
                    "Authors"
                }
                Link {
                    to: Route::SeriesIndex {},
                    class: if is_series { "on" } else { "" },
                    "Series"
                }
                // Inert placeholder until the Stats page lands. `<span>` (not
                // `<a>`) so assistive tech doesn't announce it as a link.
                span { class: "disabled", aria_disabled: "true", "Stats" }
            }
            if !on_settings {
                SearchPaletteHost {}
            }
            div { class: "atrium-actions",
                button {
                    class: "btn primary sm",
                    r#type: "button",
                    "data-testid": "add-books-button",
                    "Add books"
                }
                ThemeToggle {}
                Link {
                    to: Route::Settings {},
                    class: "btn ghost sm",
                    "Settings"
                }
                AuthControl {}
            }
        }
    }
}

/// Right-side auth slot. Renders nothing on SSR / first paint, then
/// reflects the live session: a `Log out` button for an authenticated
/// user, a `Log in` link otherwise. Logging out POSTs `/api/auth/logout`,
/// clears the session cookie server-side, and routes back to the login
/// page. Web-only because mobile uses `BottomNav` and a different auth
/// flow (bearer token in `data::token_store`).
#[cfg(any(feature = "web", feature = "server"))]
#[component]
fn AuthControl() -> Element {
    // `mut` is unused on SSR-only builds; the signal is only `.set()` from
    // the `web`-gated branches below.
    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut authed = use_signal(|| Option::<bool>::None);

    // On the web client only, ping `/api/auth/me` once after hydration so
    // the nav reflects the actual session. SSR renders with `None`, then
    // hydration overwrites — keeps the SSR markup deterministic.
    #[cfg(feature = "web")]
    use_effect(move || {
        spawn(async move {
            match crate::data::current_user().await {
                Ok(Some(_)) => authed.set(Some(true)),
                Ok(None) => authed.set(Some(false)),
                Err(_) => authed.set(Some(false)),
            }
        });
    });

    let nav = use_navigator();
    let on_logout = move |_| {
        #[cfg(feature = "web")]
        {
            spawn(async move {
                let _ = crate::data::logout().await;
                authed.set(Some(false));
                nav.replace(Route::Login {});
            });
        }
        #[cfg(not(feature = "web"))]
        {
            // SSR / server-only build: button is never clicked at runtime.
            let _ = (nav, authed);
        }
    };

    match authed() {
        Some(true) => rsx! {
            button {
                class: "btn ghost sm",
                "data-testid": "logout-button",
                r#type: "button",
                onclick: on_logout,
                "Log out"
            }
        },
        Some(false) => rsx! {
            Link {
                to: Route::Login {},
                class: "btn ghost sm",
                "Log in"
            }
        },
        None => rsx! {},
    }
}

#[cfg(not(any(feature = "web", feature = "server")))]
#[component]
fn AuthControl() -> Element {
    rsx! {}
}
