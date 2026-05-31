//! Shared Dioxus components for `omnibus` (web) and `omnibus-mobile` (native).
//!
//! Platform-specific behavior (nav variant, data-fetching transport) is
//! gated behind the `web` and `mobile` features. Components themselves stay
//! platform-agnostic — they use `use_signal` + `use_effect`, and the `data`
//! module provides a feature-gated transport layer.

use dioxus::prelude::*;

pub mod components;
pub mod contexts;
pub mod data;
pub mod pages;
pub mod reader_progress;
pub mod routes;
pub mod rpc;
pub mod styles;
pub mod view_prefs;

pub use components::Nav;
pub use contexts::*;
pub use pages::{
    AuthorPage, AuthorsIndexPage, BookDetailPage, BookReadPage, LandingPage, LoginPage,
    MetadataEditPage, RegisterPage, SearchPage, SeriesIndexPage, SeriesPage, SettingsPage,
    TagCloudPage,
};
pub use routes::*;
pub use styles::ALL_STYLES;

#[cfg(feature = "mobile")]
pub use data::ServerUrl;

/// Platform-specific page chrome. Web puts nav at the top of the flow;
/// mobile puts it at the bottom (via `position: fixed`).
///
/// The web variant is the default (compiled both for the WASM client and
/// for server-side SSR) so the SSR'd markup matches what the WASM client
/// expects to hydrate.
#[cfg(not(feature = "mobile"))]
#[component]
fn ScreenLayout(children: Element) -> Element {
    // #57: web-side reactive redirect to /login on 401. Mirrors the
    // mobile ScreenLayout's `token_store::subscribe()` loop, but driven
    // by `data::web_auth_state` since web auth lives in a session cookie
    // (no client-side token to clear). Render path stays unconditional
    // so SSR and WASM produce identical markup — only the effect runs
    // on the WASM client. `Login` / `Register` routes don't go through
    // `ScreenLayout`, so they stay reachable for unauthenticated users
    // and the redirect can't loop.
    #[cfg(feature = "web")]
    {
        let nav = dioxus_router::use_navigator();
        let mut unauthorized = use_signal(|| false);
        use_future(move || async move {
            let mut rx = data::web_auth_state::subscribe();
            // Sync the initial value once before awaiting changes. The
            // signal's initial closure ran at scope-creation time, which
            // can race with a 401 that fired (e.g. during SSR-to-WASM
            // hydration) between channel creation and this future
            // starting — without this borrow_and_update the first 401
            // is lost. Mirrors the mobile ScreenLayout pattern.
            if !*rx.borrow_and_update() {
                unauthorized.set(true);
                return;
            }
            while rx.changed().await.is_ok() {
                if !*rx.borrow_and_update() {
                    unauthorized.set(true);
                    break;
                }
            }
        });
        use_effect(move || {
            if unauthorized() {
                nav.replace(Route::Login {});
            }
        });
    }

    rsx! {
        div { class: "app-shell",
            Nav {}
            main { {children} }
        }
    }
}

#[cfg(feature = "mobile")]
#[component]
fn ScreenLayout(children: Element) -> Element {
    // Mobile auth gate. Two layers:
    //
    // * **Render-path placeholder.** When `authed` is false we render an
    //   empty screen instead of `{children}`. This is the no-flash
    //   guarantee — protected pages never mount and never kick off a
    //   data-fetch effect that would 401.
    // * **Reactive redirect.** `authed` is a Dioxus `Signal` driven by
    //   the `data::token_store::subscribe()` watch channel. When the
    //   token gets cleared mid-session (e.g. `data::note_status` on a
    //   401), the worker pushes `false`, the `use_future` loop updates
    //   the signal, the component re-renders, and the `use_effect`
    //   (which now reads a reactive signal) fires `nav.replace`.
    //
    // The auth-shell screens (`Login` / `Register`) don't go through
    // `ScreenLayout`, so they stay reachable for unauthenticated users.
    let nav = dioxus_router::use_navigator();
    let mut authed = use_signal(|| data::token_store::get().is_some());

    use_future(move || async move {
        let mut rx = data::token_store::subscribe();
        // Sync initial value once before awaiting changes — the signal's
        // initial closure ran at scope-creation time, which can race with
        // a token write that happens between scope creation and this
        // future starting.
        let current = *rx.borrow_and_update();
        if current != authed() {
            authed.set(current);
        }
        while rx.changed().await.is_ok() {
            let now = *rx.borrow_and_update();
            if now != authed() {
                authed.set(now);
            }
        }
    });

    use_effect(move || {
        if !authed() {
            nav.replace(Route::Login {});
        }
    });

    if !authed() {
        return rsx! { div { class: "screen" } };
    }
    rsx! {
        div { class: "screen",
            {children}
            Nav {}
        }
    }
}

/// Atrium design-system stylesheet (F1.7). Served as a hashed static asset
/// via Dioxus's Manganis pipeline so the browser caches it independently of
/// the WASM bundle.
const ATRIUM_CSS: Asset = asset!("/assets/atrium.css");

/// Root app component. Renders global styles and the router.
#[component]
pub fn App() -> Element {
    use_context_provider(|| SearchQuery(Signal::new(String::new())));
    #[cfg(not(feature = "mobile"))]
    use_context_provider(|| components::search_palette::PaletteOpen(Signal::new(false)));

    // Cached `/api/auth/me`. Provided unconditionally on non-mobile builds
    // so SSR can render the placeholder topbar without resolving anything;
    // the WASM hydration phase then runs the boot effect below to fill it
    // in once for the lifetime of the App instance.
    #[cfg(not(feature = "mobile"))]
    {
        use_context_provider(|| CurrentUser(Signal::new(None)));
    }

    // Single boot-time `/me` fetch (replaces the per-component effects in
    // user_menu / landing / author). Also subscribes to `web_auth_state`
    // so a fresh login refills the cache and a 401 clears it without
    // requiring a hard reload.
    #[cfg(feature = "web")]
    {
        let mut slot = use_context::<CurrentUser>().0;
        use_future(move || async move {
            // Initial fetch on mount. Only an explicit `Ok(_)` updates
            // state — transient errors (network blip, rate-limit 429)
            // leave the signal at `None` so callers keep showing the
            // pre-resolve placeholder.
            if let Ok(resolved) = data::current_user().await {
                slot.set(Some(resolved));
            }

            // React to subsequent auth-state transitions. `current_user`
            // itself pings `web_auth_state` on every call (true on 200,
            // false on 401), so the very first transition we observe
            // here is usually `true -> true` from the initial fetch
            // above — skip same-value updates to avoid a redundant
            // refetch loop.
            let mut rx = data::web_auth_state::subscribe();
            let mut last = *rx.borrow_and_update();
            while rx.changed().await.is_ok() {
                let now = *rx.borrow_and_update();
                if now == last {
                    continue;
                }
                last = now;
                if now {
                    // Fresh login (channel flipped false -> true): refetch
                    // so the avatar / admin gates update without a reload.
                    if let Ok(resolved) = data::current_user().await {
                        slot.set(Some(resolved));
                    }
                } else {
                    // 401 observed elsewhere: flip to unauthenticated
                    // immediately. ScreenLayout already drives the
                    // /login redirect off the same channel.
                    slot.set(Some(None));
                }
            }
        });
    }

    components::atrium::init_theme();
    rsx! {
        document::Title { "Omnibus" }
        document::Stylesheet { href: ATRIUM_CSS }
        for chunk in ALL_STYLES.iter() {
            style { {*chunk} }
        }
        components::atrium::AtriumRoot {
            dioxus_router::Router::<Route> {}
        }
    }
}
