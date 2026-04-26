//! Shared Dioxus components for `omnibus` (web) and `omnibus-mobile` (native).
//!
//! Platform-specific behavior (nav variant, data-fetching transport) is
//! gated behind the `web` and `mobile` features. Components themselves stay
//! platform-agnostic — they use `use_signal` + `use_effect`, and the `data`
//! module provides a feature-gated transport layer.

use dioxus::prelude::*;
use dioxus_router::Routable;

pub mod components;
pub mod data;
pub mod pages;
pub mod rpc;

pub use components::Nav;
pub use pages::{BookDetailPage, LandingPage, LoginPage, RegisterPage, SettingsPage};

#[cfg(feature = "mobile")]
pub use data::ServerUrl;

/// Top-level router for every omnibus frontend target.
#[derive(Clone, Debug, PartialEq, Eq, Routable)]
pub enum Route {
    #[route("/")]
    Landing {},
    #[route("/settings")]
    Settings {},
    #[route("/books/:id")]
    BookDetail { id: i64 },
    #[route("/login")]
    Login {},
    #[route("/register")]
    Register {},
}

/// Route target for `/` — wraps [`LandingPage`] in the platform screen layout.
#[component]
pub fn Landing() -> Element {
    rsx! {
        ScreenLayout { LandingPage {} }
    }
}

/// Route target for `/settings` — wraps [`SettingsPage`] in the platform screen layout.
#[component]
pub fn Settings() -> Element {
    rsx! {
        ScreenLayout { SettingsPage {} }
    }
}

/// Route target for `/books/:id` — stub detail page. Replace the id shape
/// once the backend exposes stable book ids.
#[component]
pub fn BookDetail(id: i64) -> Element {
    rsx! {
        ScreenLayout { BookDetailPage { id } }
    }
}

/// Route target for `/login` — credential entry form. Rendered without the
/// main screen chrome so the login flow stands alone.
#[component]
pub fn Login() -> Element {
    rsx! {
        div { class: "auth-shell",
            LoginPage {}
        }
    }
}

/// Route target for `/register` — account-creation form. Same chrome as
/// [`Login`] so the two pages feel like one flow.
#[component]
pub fn Register() -> Element {
    rsx! {
        div { class: "auth-shell",
            RegisterPage {}
        }
    }
}

/// Platform-specific page chrome. Web puts nav at the top of the flow;
/// mobile puts it at the bottom (via `position: fixed`).
///
/// The web variant is the default (compiled both for the WASM client and
/// for server-side SSR) so the SSR'd markup matches what the WASM client
/// expects to hydrate.
#[cfg(not(feature = "mobile"))]
#[component]
fn ScreenLayout(children: Element) -> Element {
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

/// Return the base URL for API calls. Mobile reads it from the `ServerUrl`
/// context; web co-locates with the server so the base is empty/relative.
pub fn use_server_url() -> String {
    #[cfg(feature = "mobile")]
    {
        use_context::<data::ServerUrl>().0
    }
    #[cfg(not(feature = "mobile"))]
    {
        String::new()
    }
}

/// Root app component. Renders global styles and the router.
#[component]
pub fn App() -> Element {
    rsx! {
        document::Title { "Omnibus" }
        style { {STYLES} }
        dioxus_router::Router::<Route> {}
    }
}

/// Global CSS — merged from the former web and mobile style sheets.
/// Selectors used by both targets live here; platform-specific adjustments
/// (e.g. `.bottom-nav` positioning) are scoped via class names.
pub const STYLES: &str = r#"
:root { color-scheme: dark; }

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  background: radial-gradient(circle at top, #1f2937 0%, #0b1020 50%, #070b16 100%);
  min-height: 100vh;
  color: #e5e7eb;
}

.app-shell {
  max-width: 1400px;
  margin: 0 auto;
  padding: 2rem clamp(1rem, 4vw, 2.5rem);
}

.screen {
  display: flex;
  flex-direction: column;
  min-height: 100vh;
  padding: 1.5rem 1rem 5rem;
}

.auth-shell {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 2rem 1rem;
}
.auth-card { width: 100%; max-width: 380px; }
.auth-form { display: flex; flex-direction: column; gap: 1rem; margin-top: 1rem; }
.auth-footer { font-size: 0.85rem; color: #94a3b8; margin-top: 1rem; text-align: center; }
.auth-footer a { color: #22d3ee; }

.top-nav {
  display: flex;
  gap: 1rem;
  margin-bottom: 1.5rem;
}
.top-nav a, .top-nav .top-nav-btn {
  color: #cbd5e1;
  text-decoration: none;
  padding: 0.4rem 0.75rem;
  border-radius: 8px;
  background: rgba(30, 41, 59, 0.7);
  border: 0;
  font: inherit;
  cursor: pointer;
}
.top-nav a:hover, .top-nav .top-nav-btn:hover { background: rgba(51, 65, 85, 0.9); }
.top-nav .top-nav-btn { margin-left: auto; }

.bottom-nav {
  position: fixed;
  bottom: 0; left: 0; right: 0;
  display: flex;
  background: rgba(15, 23, 42, 0.95);
  border-top: 1px solid rgba(100, 116, 139, 0.3);
  padding-bottom: env(safe-area-inset-bottom);
}
.bottom-nav a {
  flex: 1;
  padding: 0.9rem;
  text-align: center;
  color: #94a3b8;
  text-decoration: none;
  font-size: 0.9rem;
}
.bottom-nav a.active { color: #22d3ee; }

.card {
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.3);
  border-radius: 14px;
  padding: 1.5rem;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
}

h1 { font-size: 1.4rem; margin-bottom: 0.5rem; }
.subtitle { color: #94a3b8; margin-bottom: 1rem; }
.value-line { font-size: 1.25rem; margin-bottom: 1rem; }

.btn {
  display: block;
  margin-top: 0.75rem;
  border: 0;
  border-radius: 10px;
  background: linear-gradient(135deg, #22d3ee, #3b82f6);
  color: #031525;
  font-weight: 600;
  font-size: 1rem;
  padding: 0.7rem 1rem;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
  transition: filter 0.1s, transform 0.1s;
}
.btn:hover { filter: brightness(1.08); }
.btn:active { filter: brightness(0.85); transform: scale(0.98); }

.settings-form {
  display: flex;
  flex-direction: column;
  gap: 1.25rem;
  margin-top: 1.25rem;
}
.settings-field { display: flex; flex-direction: column; gap: 0.4rem; }
.settings-field label, .settings-label {
  font-size: 0.875rem;
  font-weight: 500;
  color: #cbd5e1;
}
.settings-field input[type="text"], .settings-input {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 8px;
  color: #e5e7eb;
  font-size: 0.95rem;
  padding: 0.55rem 0.75rem;
  width: 100%;
}
.settings-field input[type="text"]:focus, .settings-input:focus {
  outline: none;
  border-color: #3b82f6;
}

.settings-status { font-size: 0.875rem; margin-top: 0.5rem; min-height: 1.2em; }
.settings-status.success, .success-msg { color: #34d399; }
.settings-status.error, .error { color: #f87171; font-size: 0.85rem; }

.library-card { margin-top: 1.25rem; }
.library-card h2, .library-title {
  font-size: 1rem;
  font-weight: 600;
  margin-bottom: 0.75rem;
  color: #cbd5e1;
}
.library-path { font-size: 0.8rem; color: #64748b; font-family: monospace; margin-bottom: 0.4rem; }
.library-count { font-size: 0.85rem; color: #94a3b8; margin-bottom: 0.5rem; }
.library-loading, .library-empty { color: #64748b; font-size: 0.875rem; }

.library-file-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  max-height: 320px;
  overflow-y: auto;
}
.library-file-list li, .library-file {
  font-size: 0.875rem;
  font-family: monospace;
  padding: 0.3rem 0.5rem;
  background: rgba(30, 41, 59, 0.5);
  border-radius: 6px;
  color: #e2e8f0;
}

.ebook-table-wrap {
  margin-top: 1.25rem;
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.3);
  border-radius: 14px;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
  overflow-x: auto;
}
.ebook-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.875rem;
  table-layout: auto;
}
.ebook-table td,
.ebook-table th { white-space: nowrap; }
.ebook-table .ebook-col-title { white-space: normal; }
.ebook-table .ebook-title-cell {
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}
.ebook-table thead th {
  text-align: left;
  padding: 0.75rem 0.9rem;
  color: #94a3b8;
  font-weight: 600;
  font-size: 0.8rem;
  text-transform: uppercase;
  letter-spacing: 0.03em;
  border-bottom: 1px solid rgba(100, 116, 139, 0.3);
  background: rgba(15, 23, 42, 0.9);
  position: sticky;
  top: 0;
}
.ebook-table tbody td {
  padding: 0.6rem 0.9rem;
  border-bottom: 1px solid rgba(100, 116, 139, 0.15);
  color: #cbd5e1;
  vertical-align: middle;
}
.ebook-row {
  cursor: pointer;
  transition: background 0.1s;
}
.ebook-row:hover { background: rgba(51, 65, 85, 0.4); }
.ebook-row:focus-visible {
  outline: 2px solid #22d3ee;
  outline-offset: -2px;
  background: rgba(51, 65, 85, 0.4);
}
.ebook-row:last-child td { border-bottom: 0; }

.ebook-col-cover { width: 56px; }
.ebook-thumb {
  width: 40px;
  height: 60px;
  object-fit: cover;
  border-radius: 4px;
  display: block;
  background: rgba(30, 41, 59, 0.6);
}
.ebook-thumb-fallback {
  display: flex;
  align-items: center;
  justify-content: center;
  color: #475569;
  font-size: 0.75rem;
}
.ebook-col-title { min-width: 220px; }
.ebook-title-cell {
  color: #f1f5f9;
  font-weight: 600;
}

@media (max-width: 1100px) {
  .ebook-table .ebook-col-language { display: none; }
}
@media (max-width: 900px) {
  .ebook-table .ebook-col-published { display: none; }
}
@media (max-width: 720px) {
  .ebook-table .ebook-col-publisher { display: none; }
}
@media (max-width: 560px) {
  .ebook-table .ebook-col-series { display: none; }
  .ebook-table thead th,
  .ebook-table tbody td { padding: 0.5rem 0.6rem; }
  .ebook-thumb { width: 32px; height: 48px; }
}
"#;
