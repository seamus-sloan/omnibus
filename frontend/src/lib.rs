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

#[cfg(feature = "server")]
pub mod db;
#[cfg(feature = "server")]
pub mod ebook;
#[cfg(feature = "server")]
pub mod scanner;

pub use components::Nav;
pub use pages::{LandingPage, SettingsPage};

#[cfg(feature = "mobile")]
pub use data::ServerUrl;

/// Top-level router for every omnibus frontend target.
#[derive(Clone, Debug, PartialEq, Eq, Routable)]
pub enum Route {
    #[route("/")]
    Landing {},
    #[route("/settings")]
    Settings {},
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
  max-width: 760px;
  margin: 0 auto;
  padding: 2rem 1rem;
}

.screen {
  display: flex;
  flex-direction: column;
  min-height: 100vh;
  padding: 1.5rem 1rem 5rem;
}

.top-nav {
  display: flex;
  gap: 1rem;
  margin-bottom: 1.5rem;
}
.top-nav a {
  color: #cbd5e1;
  text-decoration: none;
  padding: 0.4rem 0.75rem;
  border-radius: 8px;
  background: rgba(30, 41, 59, 0.7);
}
.top-nav a:hover { background: rgba(51, 65, 85, 0.9); }

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

.ebook-grid {
  display: flex;
  flex-direction: column;
  gap: 1rem;
  margin-top: 1.25rem;
}
.ebook-card {
  display: grid;
  grid-template-columns: 140px 1fr;
  gap: 1.25rem;
  padding: 1.25rem;
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.3);
  border-radius: 14px;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
}
.ebook-cover {
  width: 140px;
  height: 210px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(30, 41, 59, 0.6);
  border-radius: 8px;
  overflow: hidden;
}
.ebook-cover img {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
}
.ebook-cover-fallback {
  color: #64748b;
  font-size: 0.85rem;
}
.ebook-info { min-width: 0; }
.ebook-title {
  font-size: 1.1rem;
  font-weight: 600;
  margin-bottom: 0.3rem;
  color: #f1f5f9;
}
.ebook-authors {
  font-size: 0.95rem;
  color: #cbd5e1;
  margin-bottom: 0.5rem;
}
.ebook-series {
  font-size: 0.85rem;
  color: #22d3ee;
  margin-bottom: 0.4rem;
}
.ebook-meta {
  font-size: 0.8rem;
  color: #94a3b8;
  margin-bottom: 0.2rem;
}
.ebook-subjects {
  font-size: 0.75rem;
  color: #64748b;
  margin: 0.4rem 0;
  font-style: italic;
}
.ebook-description {
  font-size: 0.85rem;
  color: #cbd5e1;
  margin-top: 0.6rem;
  line-height: 1.4;
  max-height: 8rem;
  overflow-y: auto;
}

.ebook-fields {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 0.2rem 0.75rem;
  margin: 0.6rem 0;
  font-size: 0.8rem;
}
.ebook-fields dt { color: #64748b; }
.ebook-fields dd { color: #cbd5e1; margin: 0; word-break: break-word; }

.ebook-counts { color: #64748b; font-size: 0.75rem; margin-top: 0.3rem; }

.ebook-ids {
  list-style: none;
  padding: 0;
  margin: 0.2rem 0 0.5rem;
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  font-size: 0.8rem;
}
.ebook-ids code { color: #e2e8f0; }
.ebook-id-scheme { color: #22d3ee; font-size: 0.75rem; }

.ebook-raw {
  margin-top: 0.75rem;
  padding-top: 0.75rem;
  border-top: 1px solid rgba(100, 116, 139, 0.2);
}
.ebook-raw summary {
  cursor: pointer;
  color: #94a3b8;
  font-size: 0.8rem;
  padding: 0.2rem 0;
}
.ebook-raw summary:hover { color: #e2e8f0; }

.raw-meta-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.75rem;
  margin-top: 0.5rem;
}
.raw-meta-table th,
.raw-meta-table td {
  text-align: left;
  padding: 0.35rem 0.5rem;
  border-bottom: 1px solid rgba(100, 116, 139, 0.15);
  vertical-align: top;
}
.raw-meta-table th { color: #94a3b8; font-weight: 600; }
.raw-meta-table td code { color: #22d3ee; font-family: monospace; }
.raw-meta-value { color: #e2e8f0; word-break: break-word; max-width: 400px; }
.raw-meta-lang { color: #64748b; font-size: 0.7rem; }
.raw-meta-empty { color: #475569; }
.raw-meta-refs {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.1rem;
  color: #cbd5e1;
}

@media (max-width: 560px) {
  .ebook-card { grid-template-columns: 100px 1fr; gap: 0.75rem; padding: 0.9rem; }
  .ebook-cover { width: 100px; height: 150px; }
  .raw-meta-value { max-width: none; }
}
"#;
