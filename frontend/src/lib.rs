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
pub mod view_prefs;

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

/// Cross-route search query. Owned by [`App`] via `use_context_provider`
/// so the [`Nav`]-hosted search box and the [`LandingPage`] read/write the
/// same signal — typing in the nav from any route updates the landing
/// results without a route-param round-trip.
#[derive(Copy, Clone)]
pub struct SearchQuery(pub Signal<String>);

/// Convenience accessor for the search-query context.
pub fn use_search_query() -> SearchQuery {
    use_context::<SearchQuery>()
}

/// Atrium design-system stylesheet (F1.7). Served as a hashed static asset
/// via Dioxus's Manganis pipeline so the browser caches it independently of
/// the WASM bundle.
const ATRIUM_CSS: Asset = asset!("/assets/atrium.css");

/// Root app component. Renders global styles and the router.
#[component]
pub fn App() -> Element {
    use_context_provider(|| SearchQuery(Signal::new(String::new())));
    components::atrium::init_theme();
    rsx! {
        document::Title { "Omnibus" }
        document::Stylesheet { href: ATRIUM_CSS }
        style { {STYLES} }
        components::atrium::AtriumRoot {
            dioxus_router::Router::<Route> {}
        }
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
  background-attachment: fixed;
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

/* ---- F1.6 auth primitives (AuthShell / Field / Banner / StrengthMeter) ----
   Self-contained block. Tokens are `--auth-*` prefixed and declared on
   `:root` so each primitive renders correctly whether or not it's nested
   under an AuthShell. No effect on the rest of the app — nothing outside
   this block references the prefixed tokens. */
:root {
  --auth-ink-0: #f8fafc;
  --auth-ink-1: #cbd5e1;
  --auth-ink-2: #94a3b8;
  --auth-ink-3: #64748b;
  --auth-bg-0: rgba(15, 23, 42, 0.92);
  --auth-bg-1: rgba(15, 23, 42, 0.65);
  --auth-bg-2: rgba(30, 41, 59, 0.5);
  --auth-line: rgba(100, 116, 139, 0.25);
  --auth-line-2: rgba(100, 116, 139, 0.45);
  --auth-accent: #22d3ee;
  --auth-accent-ink: #03131c;
  --auth-ok: #34d399;
  --auth-warn: #fbbf24;
  --auth-bad: #f87171;
  --auth-info: #60a5fa;
  --auth-sans: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  --auth-serif: "Iowan Old Style", "Palatino Linotype", Palatino, Georgia, serif;
  --auth-mono: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
}

.auth-shell-grid {
  display: grid;
  grid-template-columns: 1.1fr 1fr;
  min-height: 100vh;
  color: var(--auth-ink-0);
  font-family: var(--auth-sans);
}
@media (max-width: 720px) {
  .auth-shell-grid { grid-template-columns: 1fr; }
  .auth-shell-art { display: none; }
}

.auth-shell-art {
  background: var(--auth-bg-1);
  border-right: 1px solid var(--auth-line-2);
  padding: 3rem 3.5rem;
  display: flex;
  flex-direction: column;
  position: relative;
  overflow: hidden;
}
.auth-shell-brand { display: flex; align-items: center; gap: 0.6rem; }
.auth-shell-brand-mark {
  width: 22px;
  height: 22px;
  border-radius: 6px;
  background: linear-gradient(135deg, var(--auth-accent), #3b82f6);
}
.auth-shell-brand-word {
  font-family: var(--auth-serif);
  font-size: 1.25rem;
  letter-spacing: 0.04em;
}

.auth-shell-tagline { margin-top: auto; max-width: 460px; }
.auth-shell-headline {
  font-family: var(--auth-serif);
  font-size: clamp(2.4rem, 5vw, 3.5rem);
  line-height: 1;
  margin: 0;
}
.auth-shell-headline-em { font-style: italic; }
.auth-shell-blurb {
  margin-top: 1rem;
  color: var(--auth-ink-1);
  font-size: 0.95rem;
  line-height: 1.55;
}
.auth-shell-meta {
  margin-top: 1.4rem;
  display: flex;
  gap: 1rem;
  font-family: var(--auth-mono);
  font-size: 0.7rem;
  color: var(--auth-ink-3);
  text-transform: uppercase;
  letter-spacing: 0.14em;
}

.auth-shell-form {
  display: grid;
  place-items: center;
  padding: 3rem 2rem;
}
.auth-shell-form-inner { width: 100%; max-width: 420px; }
.auth-shell-kicker {
  font-family: var(--auth-mono);
  font-size: 0.72rem;
  color: var(--auth-ink-3);
  text-transform: uppercase;
  letter-spacing: 0.14em;
}
.auth-shell-title {
  margin: 0.4rem 0 0;
  font-family: var(--auth-serif);
  font-size: 1.9rem;
  line-height: 1.15;
}
.auth-shell-lede {
  margin-top: 0.7rem;
  color: var(--auth-ink-2);
  font-size: 0.9rem;
  line-height: 1.55;
}
.auth-shell-body { margin-top: 1.6rem; }

/* Field */
.auth-field { display: block; margin-bottom: 0.9rem; }
.auth-field-label-row {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  margin-bottom: 0.35rem;
}
.auth-field-label {
  font-family: var(--auth-mono);
  font-size: 0.72rem;
  color: var(--auth-ink-2);
  text-transform: uppercase;
  letter-spacing: 0.1em;
}
.auth-field-action { font-size: 0.78rem; color: var(--auth-accent); }
.auth-field-input-wrap { position: relative; }
.auth-field input,
.auth-field textarea,
.auth-field select {
  width: 100%;
  padding: 0.75rem 0.9rem;
  background: var(--auth-bg-1);
  border: 1px solid var(--auth-line-2);
  border-radius: 10px;
  color: var(--auth-ink-0);
  font-size: 0.9rem;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s;
}
.auth-field input:focus,
.auth-field textarea:focus,
.auth-field select:focus { border-color: var(--auth-accent); }
.auth-field-err input,
.auth-field-err textarea,
.auth-field-err select {
  border-color: var(--auth-bad);
  box-shadow: 0 0 0 3px color-mix(in oklch, var(--auth-bad) 18%, transparent);
}
.auth-field-ok input,
.auth-field-ok textarea,
.auth-field-ok select {
  border-color: var(--auth-ok);
  box-shadow: 0 0 0 3px color-mix(in oklch, var(--auth-ok) 18%, transparent);
}
.auth-field-check {
  position: absolute;
  right: 0.75rem;
  top: 50%;
  transform: translateY(-50%);
  color: var(--auth-ok);
  font-size: 1rem;
  line-height: 1;
}
.auth-field-msg {
  margin-top: 0.4rem;
  font-size: 0.78rem;
  line-height: 1.4;
}
.auth-field-msg-err { color: var(--auth-bad); }
.auth-field-msg-hint {
  color: var(--auth-ink-3);
  font-family: var(--auth-mono);
  font-size: 0.7rem;
  letter-spacing: 0.02em;
}

/* Banner */
.auth-banner {
  display: flex;
  gap: 0.75rem;
  align-items: flex-start;
  padding: 0.85rem 1rem;
  margin-bottom: 1rem;
  border: 1px solid var(--auth-line-2);
  border-radius: 10px;
  background: var(--auth-bg-1);
}
.auth-banner-err   { border-left: 3px solid var(--auth-bad); }
.auth-banner-warn  { border-left: 3px solid var(--auth-warn); }
.auth-banner-info  { border-left: 3px solid var(--auth-info); }
.auth-banner-ok    { border-left: 3px solid var(--auth-ok); }
.auth-banner-icon {
  width: 20px;
  height: 20px;
  display: grid;
  place-items: center;
  border-radius: 999px;
  font-weight: 700;
  font-size: 0.75rem;
  flex: 0 0 20px;
  background: var(--auth-bg-2);
}
.auth-banner-err  .auth-banner-icon { color: var(--auth-bad); }
.auth-banner-warn .auth-banner-icon { color: var(--auth-warn); }
.auth-banner-info .auth-banner-icon { color: var(--auth-info); }
.auth-banner-ok   .auth-banner-icon { color: var(--auth-ok); }
.auth-banner-body { flex: 1; }
.auth-banner-title { font-weight: 500; font-size: 0.85rem; color: var(--auth-ink-0); }
.auth-banner-message { margin-top: 0.25rem; font-size: 0.8rem; color: var(--auth-ink-1); line-height: 1.5; }
.auth-banner-action { margin-top: 0.6rem; display: flex; gap: 0.5rem; }
.auth-banner-dismiss {
  background: transparent;
  border: 0;
  color: var(--auth-ink-3);
  cursor: pointer;
  padding: 0.1rem 0.3rem;
  font: inherit;
  align-self: flex-start;
}
.auth-banner-dismiss:hover { color: var(--auth-ink-0); }

/* StrengthMeter */
.auth-strength { margin-top: 0.5rem; }
.auth-strength-bar {
  display: flex;
  gap: 0.25rem;
}
.auth-strength-segment {
  flex: 1;
  height: 3px;
  border-radius: 999px;
  background: var(--auth-bg-2);
}
.auth-strength-tier-bad  .auth-strength-segment-on { background: var(--auth-bad); }
.auth-strength-tier-warn .auth-strength-segment-on { background: var(--auth-warn); }
.auth-strength-tier-mid  .auth-strength-segment-on { background: #eab308; }
.auth-strength-tier-ok   .auth-strength-segment-on { background: var(--auth-ok); }
.auth-strength-label {
  margin-top: 0.4rem;
  display: flex;
  justify-content: space-between;
  font-family: var(--auth-mono);
  font-size: 0.68rem;
  color: var(--auth-ink-3);
  letter-spacing: 0.04em;
}
.auth-strength-label-rhs.auth-strength-tier-bad  { color: var(--auth-bad); }
.auth-strength-label-rhs.auth-strength-tier-warn { color: var(--auth-warn); }
.auth-strength-label-rhs.auth-strength-tier-mid  { color: #eab308; }
.auth-strength-label-rhs.auth-strength-tier-ok   { color: var(--auth-ok); }

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

.top-nav .library-search { flex: 1; min-width: 0; max-width: 480px; }
.top-nav .library-search input[type="search"] {
  width: 100%;
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 8px;
  color: #e5e7eb;
  font: inherit;
  padding: 0.4rem 0.75rem;
}
.top-nav .library-search input[type="search"]::placeholder { color: #94a3b8; }
.top-nav .library-search input[type="search"]:focus {
  outline: none;
  border-color: #3b82f6;
}

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
.settings-field input[type="text"],
.settings-field input[type="password"],
.settings-field input[type="email"],
.settings-input {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 8px;
  color: #e5e7eb;
  font-size: 0.95rem;
  padding: 0.55rem 0.75rem;
  width: 100%;
}
.settings-field input[type="text"]:focus,
.settings-field input[type="password"]:focus,
.settings-field input[type="email"]:focus,
.settings-input:focus {
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

/* ===== Book detail page ===== */
.book-detail {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2rem;
  align-items: start;
}
@media (max-width: 600px) {
  .book-detail { grid-template-columns: 1fr; }
}
.book-detail-cover { width: 220px; max-width: 100%; }
.book-detail-cover img {
  width: 100%; height: auto; display: block;
  border-radius: 6px; box-shadow: 0 4px 20px rgba(0,0,0,.5);
}
.book-detail-cover-fallback {
  width: 220px; height: 300px; background: rgba(255,255,255,.05);
  border-radius: 6px; display: flex; align-items: center;
  justify-content: center; font-size: 3rem; color: rgba(255,255,255,.2);
}
.book-detail-meta { display: flex; flex-direction: column; gap: .5rem; min-width: 0; }
.breadcrumb {
  display: flex; gap: .5rem; align-items: center;
  font-size: .85rem; color: rgba(255,255,255,.5); margin-bottom: .5rem;
}
.breadcrumb a { color: #22d3ee; text-decoration: none; }
.breadcrumb a:hover { text-decoration: underline; }
.book-detail-description { line-height: 1.6; color: rgba(255,255,255,.8); margin: .5rem 0 1rem; }
.format-actions { display: flex; gap: .75rem; flex-wrap: wrap; margin: .75rem 0; }
.tag-list { display: flex; flex-wrap: wrap; gap: .4rem; list-style: none; padding: 0; margin: .4rem 0; }
.tag {
  background: rgba(34,211,238,.12); border: 1px solid rgba(34,211,238,.3);
  border-radius: 9999px; padding: .2rem .65rem; font-size: .8rem; color: #22d3ee;
}
.identifier-list {
  display: grid; grid-template-columns: auto 1fr;
  gap: .2rem .75rem; font-size: .85rem; margin: .5rem 0;
}
.identifier-list dt { color: rgba(255,255,255,.5); font-family: monospace; }
.identifier-list dd { margin: 0; font-family: monospace; }
.ratings-slot, .suggestions-slot { min-height: 1px; }

/* ===== F1.3 — Library views (toolbar / sidebar / grid / chips) ===== */

.lib-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.75rem;
  margin-top: 1rem;
  padding: 0.5rem 0.25rem;
}
.lib-view-toggle {
  display: inline-flex;
  background: rgba(15, 23, 42, 0.7);
  border: 1px solid rgba(100, 116, 139, 0.3);
  border-radius: 8px;
  overflow: hidden;
}
.lib-toggle-btn {
  background: transparent;
  border: 0;
  color: #cbd5e1;
  font: inherit;
  padding: 0.4rem 0.9rem;
  cursor: pointer;
}
.lib-toggle-btn[aria-pressed="true"] {
  background: linear-gradient(135deg, #22d3ee, #3b82f6);
  color: #031525;
  font-weight: 600;
}
.lib-sort-controls { display: inline-flex; align-items: center; gap: 0.5rem; margin-left: auto; }
.lib-sort-label {
  display: inline-flex; align-items: center; gap: 0.4rem;
  color: #94a3b8; font-size: 0.85rem;
}
.lib-sort-select {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 6px;
  color: #e5e7eb;
  font: inherit;
  padding: 0.35rem 0.55rem;
}
.lib-sort-dir {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 6px;
  color: #e5e7eb;
  font: inherit;
  padding: 0.35rem 0.65rem;
  cursor: pointer;
}

.lib-layout {
  display: grid;
  grid-template-columns: 210px 1fr;
  gap: 1.25rem;
  margin-top: 1rem;
  align-items: start;
}
.lib-layout--collapsed {
  grid-template-columns: 1fr;
}
.lib-layout--collapsed > .lib-sidebar { display: none; }

/* Narrow viewports: layout always single-column. The sidebar becomes a
   right-edge drawer overlay when open, so it doesn't squeeze the grid. */
@media (max-width: 900px) {
  .lib-layout { grid-template-columns: 1fr; }
  .lib-layout > .lib-sidebar {
    position: fixed;
    top: 4rem;
    right: 0.75rem;
    z-index: 50;
    width: min(280px, calc(100vw - 1.5rem));
    max-height: calc(100vh - 5rem);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.55);
  }
}

/* Filters toggle in the toolbar: same pill shape as the view-mode pair,
   but a standalone button (no enclosing toggle group). */
.lib-filters-btn {
  background: rgba(15, 23, 42, 0.7);
  border: 1px solid rgba(100, 116, 139, 0.3);
  border-radius: 8px;
  padding: 0.4rem 0.9rem;
}
.lib-filters-btn[aria-pressed="true"] {
  background: linear-gradient(135deg, #22d3ee, #3b82f6);
  color: #031525;
  font-weight: 600;
  border-color: transparent;
}

.lib-sidebar {
  background: rgba(15, 23, 42, 0.7);
  border: 1px solid rgba(100, 116, 139, 0.25);
  border-radius: 12px;
  padding: 1rem;
  display: flex;
  flex-direction: column;
  gap: 1rem;
  position: sticky;
  top: 1rem;
  max-height: calc(100vh - 2rem);
  overflow-y: auto;
}
.lib-clear-filters {
  align-self: flex-start;
  background: transparent;
  border: 1px solid rgba(100, 116, 139, 0.4);
  color: #cbd5e1;
  border-radius: 9999px;
  padding: 0.25rem 0.7rem;
  font: inherit;
  font-size: 0.8rem;
  cursor: pointer;
}
.lib-clear-filters:hover { background: rgba(51, 65, 85, 0.5); }

.lib-facet { display: flex; flex-direction: column; gap: 0.5rem; }
.lib-facet-title {
  font-size: 0.8rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: #94a3b8;
  font-weight: 600;
}
.lib-chip-list {
  list-style: none;
  display: flex;
  flex-wrap: wrap;
  gap: 0.35rem;
  padding: 0;
  margin: 0;
}
.lib-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  max-width: 100%;
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.35);
  border-radius: 9999px;
  padding: 0.2rem 0.6rem;
  color: #cbd5e1;
  font: inherit;
  font-size: 0.8rem;
  cursor: pointer;
  /* Long author names / sentence-y tags shouldn't blow out the sidebar
     or wrap into multi-line chips. The label span clips with ellipsis;
     the full value stays accessible via the chip's `title` tooltip. */
  text-align: left;
}
.lib-chip-label {
  display: inline-block;
  max-width: 11rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}
.lib-chip:hover { background: rgba(51, 65, 85, 0.7); }
.lib-chip[aria-pressed="true"] {
  background: rgba(34, 211, 238, 0.18);
  border-color: rgba(34, 211, 238, 0.55);
  color: #67e8f9;
}
.lib-chip-count {
  flex-shrink: 0;
  font-size: 0.7rem;
  color: #64748b;
  background: rgba(15, 23, 42, 0.6);
  border-radius: 9999px;
  padding: 0 0.4rem;
}
.lib-chip[aria-pressed="true"] .lib-chip-count { color: #cbd5e1; }

.lib-main { min-width: 0; }

/* Sortable column headers */
.sort-th .sort-th-btn {
  background: transparent;
  border: 0;
  color: inherit;
  font: inherit;
  text-transform: inherit;
  letter-spacing: inherit;
  cursor: pointer;
  padding: 0;
}
.sort-th[aria-sort="ascending"] .sort-th-btn,
.sort-th[aria-sort="descending"] .sort-th-btn { color: #22d3ee; }

/* Cover grid */
.lib-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 1rem;
}
.lib-tile {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  background: rgba(15, 23, 42, 0.7);
  border: 1px solid rgba(100, 116, 139, 0.25);
  border-radius: 10px;
  padding: 0.75rem;
  cursor: pointer;
  transition: transform 0.1s, background 0.1s;
}
.lib-tile:hover { background: rgba(51, 65, 85, 0.55); transform: translateY(-2px); }
.lib-tile:focus-visible { outline: 2px solid #22d3ee; outline-offset: 2px; }
.lib-tile-cover { aspect-ratio: 2 / 3; overflow: hidden; border-radius: 6px; background: rgba(30, 41, 59, 0.6); }
.lib-tile-img { width: 100%; height: 100%; object-fit: cover; display: block; }
.lib-tile-fallback {
  display: flex; align-items: center; justify-content: center;
  color: #475569; font-size: 1.5rem;
}
.lib-tile-title {
  font-weight: 600;
  color: #f1f5f9;
  font-size: 0.9rem;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}
.lib-tile-author {
  color: #94a3b8;
  font-size: 0.8rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@media (max-width: 1100px) { .ebook-table .ebook-col-updated { display: none; } }
@media (max-width: 1300px) { .ebook-table .ebook-col-added { display: none; } }
"#;
