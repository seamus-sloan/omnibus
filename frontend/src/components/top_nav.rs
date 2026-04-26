use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route, Link};

use crate::{use_search_query, Route};

#[component]
pub fn TopNav() -> Element {
    // Hide the search box on `/settings` — the page has its own dense form
    // layout and a search box wedged into the nav above it just clutters
    // the chrome.
    let on_settings = matches!(use_route::<Route>(), Route::Settings {});

    rsx! {
        nav { class: "top-nav",
            Link { to: Route::Landing {}, "Home" }
            Link { to: Route::Settings {}, "Settings" }
            if !on_settings {
                NavSearch {}
            }
            AuthControl {}
        }
    }
}

/// Site-wide search box. Owns the input wired to the [`crate::SearchQuery`]
/// context, so typing here updates the same signal the landing page reads
/// from. When the user types from a non-Landing route, navigate to `/` so
/// they actually see the matching rows.
#[component]
fn NavSearch() -> Element {
    let mut query = use_search_query().0;
    let nav = use_navigator();
    let route = use_route::<Route>();

    rsx! {
        form {
            class: "library-search",
            role: "search",
            onsubmit: move |evt| evt.prevent_default(),
            input {
                id: "library-search-input",
                "data-testid": "library-search-input",
                r#type: "search",
                aria_label: "Search books",
                placeholder: "Search title, author, series, tag, ISBN…",
                value: "{query}",
                oninput: move |evt| {
                    let v = evt.value();
                    query.set(v.clone());
                    // Off-Landing keystrokes redirect to `/` so the
                    // matching rows render. Once we navigate, the
                    // component re-renders with `route == Landing` and
                    // this branch stops firing — so editing a non-empty
                    // query from a detail route still redirects, but
                    // typing on Landing doesn't spam `replace`. Empty
                    // input is excluded so clearing the box doesn't drag
                    // the user back to `/` mid-edit.
                    if !v.is_empty() && !matches!(route, Route::Landing {}) {
                        nav.replace(Route::Landing {});
                    }
                },
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
                class: "top-nav-btn",
                "data-testid": "logout-button",
                r#type: "button",
                onclick: on_logout,
                "Log out"
            }
        },
        Some(false) => rsx! {
            Link { to: Route::Login {}, "Log in" }
        },
        None => rsx! {},
    }
}

#[cfg(not(any(feature = "web", feature = "server")))]
#[component]
fn AuthControl() -> Element {
    rsx! {}
}
