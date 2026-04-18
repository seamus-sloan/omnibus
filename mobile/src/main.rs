mod components;
mod pages;

use components::BottomNav;
use dioxus::prelude::*;
use dioxus_router::{Routable, Router};
use pages::{LandingPage, SettingsPage};

#[derive(Clone, PartialEq)]
pub struct ServerUrl(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Routable)]
enum Route {
    #[route("/")]
    Landing {},
    #[route("/settings")]
    Settings {},
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    use_context_provider(|| ServerUrl("http://127.0.0.1:3000".to_string()));

    rsx! {
        style { {STYLES} }
        Router::<Route> {}
    }
}

const STYLES: &str = r#"
:root { color-scheme: dark; }

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  background: radial-gradient(circle at top, #1f2937 0%, #0b1020 50%, #070b16 100%);
  min-height: 100vh;
  color: #e5e7eb;
}

.screen {
  display: flex;
  flex-direction: column;
  min-height: 100vh;
  padding: 1.5rem 1rem 5rem;
}

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

.error { color: #f87171; font-size: 0.85rem; margin-bottom: 0.75rem; }

.btn {
  display: block;
  width: 100%;
  border: 0;
  border-radius: 10px;
  background: linear-gradient(135deg, #22d3ee, #3b82f6);
  color: #031525;
  font-weight: 600;
  font-size: 1rem;
  padding: 0.9rem 1rem;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
  transition: filter 0.1s, transform 0.1s;
}

.btn:active { filter: brightness(0.85); transform: scale(0.98); }

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

.settings-form { display: flex; flex-direction: column; gap: 1.25rem; margin-top: 1rem; }
.settings-field { display: flex; flex-direction: column; gap: 0.4rem; }
.settings-label { font-size: 0.875rem; font-weight: 500; color: #cbd5e1; }
.settings-input {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 8px;
  color: #e5e7eb;
  font-size: 0.95rem;
  padding: 0.55rem 0.75rem;
  width: 100%;
}
.success-msg { color: #34d399; font-size: 0.85rem; margin-top: 0.75rem; }
.library-card { margin-top: 1rem; }
.library-title { font-size: 1rem; font-weight: 600; margin-bottom: 0.75rem; color: #cbd5e1; }
.library-path { font-size: 0.75rem; color: #64748b; font-family: monospace; margin-bottom: 0.4rem; }
.library-count { font-size: 0.85rem; color: #94a3b8; margin-bottom: 0.5rem; }
.library-empty { color: #64748b; font-size: 0.875rem; }
.library-file-list { display: flex; flex-direction: column; gap: 0.25rem; }
.library-file {
  font-size: 0.85rem;
  font-family: monospace;
  padding: 0.3rem 0.5rem;
  background: rgba(30, 41, 59, 0.5);
  border-radius: 6px;
  color: #e2e8f0;
}
"#;

#[component]
fn Landing() -> Element {
    rsx! {
        div { class: "screen",
            LandingPage {}
            BottomNav {}
        }
    }
}

#[component]
fn Settings() -> Element {
    rsx! {
        div { class: "screen",
            SettingsPage {}
            BottomNav {}
        }
    }
}
