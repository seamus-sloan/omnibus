//! Shared Dioxus components for `omnibus` (web) and `omnibus-mobile` (native).
//!
//! Platform-specific behavior (nav variant, data-fetching transport) is
//! gated behind the `web` and `mobile` features. Components themselves stay
//! platform-agnostic — they use `use_signal` + `use_effect`, and the `data`
//! module provides a feature-gated transport layer.

use dioxus::prelude::*;

pub mod audiobook_progress;
pub mod components;
pub mod contexts;
pub mod data;
#[cfg(feature = "mobile")]
pub(crate) mod native_share;
pub mod pages;
pub mod reader_progress;
pub mod routes;
pub mod rpc;
pub mod view_prefs;

pub use components::Nav;
pub use contexts::*;
pub use pages::{
    AccountPage, AuthorPage, AuthorsIndexPage, BookDetailPage, BookListenPage, BookReadPage,
    LandingPage, LoginPage, MetadataEditPage, RegisterPage, SearchPage, SeriesIndexPage,
    SeriesPage, SettingsPage, TagCloudPage,
};
pub use routes::*;

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
            // Persistent mini-dock. Lives in `ScreenLayout` (not the
            // immersive `/listen` / `/read` routes, which render bare) so it
            // shows on every main page while an audiobook is loaded and is
            // absent on the full player. Renders an empty host until a book
            // is playing — see `pages::MiniDock`.
            pages::MiniDock {}
            // Phone-width section switcher. Always rendered on web (SSR +
            // WASM, so hydration markup matches); CSS hides it above the
            // phone breakpoint so the desktop chrome is unchanged.
            components::BottomNav {}
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

    // Single prioritized redirect: an unconfigured server URL sends the user
    // to the Connect screen first, then an absent token to Login. Reading
    // `use_server_url()` inside the effect subscribes the effect to the
    // context signal, so a URL set on the Connect screen re-runs this.
    // `ServerConnect` / `Login` are unguarded, so neither redirect loops.
    use_effect(move || {
        if use_server_url().is_empty() {
            nav.replace(Route::ServerConnect {});
        } else if !authed() {
            nav.replace(Route::Login {});
        }
    });

    if use_server_url().is_empty() || !authed() {
        return rsx! { div { class: "screen" } };
    }
    rsx! {
        div { class: "screen",
            {children}
            Nav {}
        }
    }
}

/// Atrium design-system stylesheet. Served as a hashed static asset via
/// Dioxus's Manganis pipeline so the browser caches it independently of
/// the WASM bundle.
const ATRIUM_CSS: Asset = asset!("/assets/atrium.css");

/// Install the search-palette context and the global `⌘K` shortcut.
///
/// Called unconditionally from [`App`] so the call site has no
/// `cfg`-gated hooks — the cfg lives in the helper body. The mobile
/// build compiles to a no-op stub below; hook-count parity across
/// targets isn't claimed (the palette + shortcut aren't reachable on
/// mobile), but rule 07's invariant for SSR-vs-WASM hydration within
/// the web build is preserved.
#[cfg(not(feature = "mobile"))]
fn use_palette_setup() {
    use_context_provider(|| components::search_palette::PaletteOpen(Signal::new(false)));
    // App mounts once for the app's lifetime; registering the keydown
    // listener here keeps a single closure alive across route changes.
    components::search_palette::use_palette_global_shortcut();
}

/// Mobile stub: no palette, no global shortcut.
#[cfg(feature = "mobile")]
fn use_palette_setup() {}

/// Install the cached-user and playback contexts. Web (and SSR) only;
/// mobile uses bearer tokens via `token_store` and stubs the listen page.
///
/// Called unconditionally from [`App`] so the cfg lives in the body,
/// not at the call site. The mobile build compiles to a no-op stub
/// (the underlying `CurrentUser`/`PlaybackState` types are themselves
/// `cfg(not(mobile))` in `contexts.rs`, so true hook-count parity
/// across mobile isn't reachable without lifting those gates — out of
/// scope here). Rule 07's invariant for SSR-vs-WASM hydration within
/// the web build is preserved because both targets share `not(mobile)`.
#[cfg(not(feature = "mobile"))]
fn use_user_and_playback_contexts() {
    // Cached `/api/auth/me`. Provided unconditionally on non-mobile builds
    // so SSR can render the placeholder topbar without resolving anything;
    // the WASM hydration phase then runs the boot effect below to fill it
    // in once for the lifetime of the App instance.
    use_context_provider(|| CurrentUser(Signal::new(None)));
    // App-wide audiobook playback. Provided unconditionally on
    // not(mobile) so SSR markup matches the WASM client; the web-only
    // driver below reacts to its `uuid` signal.
    use_context_provider(PlaybackState::new);
}

/// Mobile stub: no `/me` cache, no playback context.
#[cfg(feature = "mobile")]
fn use_user_and_playback_contexts() {}

/// Install the app-root playback shim and the single boot-time `/me` fetch.
///
/// Web only: installs the `window.OmnibusAudio` shim (so the `<audio>` element
/// and playback signals outlive navigation for the persistent mini-dock) and
/// runs one `current_user()` fetch, then subscribes to `web_auth_state` so a
/// fresh login refills the cache and a 401 clears it without a hard reload.
/// Called unconditionally from [`App`]; the cfg lives here so the call site
/// stays gate-free.
#[cfg(feature = "web")]
fn use_current_user_boot() {
    // App-root audiobook playback driver: installs the `window.OmnibusAudio`
    // shim and reacts to the playback context's `uuid` signal.
    pages::install_audio_bootstrap(use_playback());

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

/// Non-web stub: no playback shim, no `/me` boot fetch.
#[cfg(not(feature = "web"))]
fn use_current_user_boot() {}

/// Patch the viewport meta on mobile so content fills the screen edge-to-edge.
///
/// Mobile (wry WKWebView) only: the Dioxus-generated viewport meta lacks
/// `viewport-fit=cover`, so the WebView insets its scroll content by the safe
/// areas (status bar + home indicator) — a phantom ~100px of scroll behind the
/// fixed full-screen player, and `env(safe-area-inset-*)` reads 0. The patch is
/// an effect (not markup), so it's free to be target-gated. Non-mobile is a
/// no-op stub; called unconditionally from [`App`].
#[cfg(feature = "mobile")]
fn use_mobile_viewport_fix() {
    use_effect(|| {
        dioxus::document::eval(
            "const m = document.querySelector('meta[name=viewport]');\
             if (m) m.setAttribute('content', 'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, viewport-fit=cover');",
        );
    });
}

/// Non-mobile stub: the viewport meta is already correct.
#[cfg(not(feature = "mobile"))]
fn use_mobile_viewport_fix() {}

/// Root app component. Renders global styles and the router.
#[component]
pub fn App() -> Element {
    use_context_provider(|| SearchQuery(Signal::new(String::new())));
    // Hook calls in App() are unconditional — the feature gates live inside
    // the helper bodies (mobile compiles them to no-op stubs). This keeps
    // rule 07's SSR-vs-WASM hydration parity within the not(mobile) build,
    // where the helpers are real hooks; true hook-count parity across mobile
    // isn't claimed (the stubs run no hooks).
    use_palette_setup();
    use_user_and_playback_contexts();
    use_current_user_boot();

    components::atrium::init_theme();

    use_mobile_viewport_fix();

    // The single `<audio>` element, mounted at the App root (sibling of the
    // Router) so it never unmounts on navigation — the persistence anchor for
    // cross-page playback. Rendered on not(mobile) for SSR/WASM hydration
    // parity; empty on mobile.
    #[cfg(not(feature = "mobile"))]
    let audio_host = rsx! { pages::AudioElement {} };
    #[cfg(feature = "mobile")]
    let audio_host = rsx! {};

    rsx! {
        document::Title { "Omnibus" }
        document::Stylesheet { href: ATRIUM_CSS }
        components::atrium::AtriumRoot {
            {audio_host}
            dioxus_router::Router::<Route> {}
        }
    }
}
