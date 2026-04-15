use dioxus::prelude::*;
use dioxus_router::prelude::Routable;

#[derive(Clone, Debug, PartialEq, Eq, Routable)]
pub enum Route {
    #[route("/", LandingPage)]
    Landing {},
    #[route("/settings", SettingsPage)]
    Settings {},
}

#[derive(Props, Clone, PartialEq)]
pub struct AppProps {
    pub route: Route,
    pub value: i64,
}

pub fn next_value_after_increment(current: i64, response_value: i64) -> i64 {
    if response_value < current {
        current
    } else {
        response_value
    }
}

#[component]
pub fn App(props: AppProps) -> Element {
    let title = match props.route {
        Route::Landing {} => "Omnibus Counter",
        Route::Settings {} => "Settings",
    };

    rsx! {
        document::Title { "{title}" }
        style { {styles()} }

        div { class: "app-shell",
            nav { class: "top-nav",
                a { href: "/", "Home" }
                a { href: "/settings", "Settings" }
            }
            main {
                match props.route {
                    Route::Landing {} => rsx! { LandingPage { value: Some(props.value) } },
                    Route::Settings {} => rsx! { SettingsPage {} },
                }
            }
        }
        script { dangerous_inner_html: "{interaction_script()}" }
    }
}

#[component]
fn LandingPage(value: Option<i64>) -> Element {
    let value = value.unwrap_or_default();
    rsx! {
        section { class: "card",
            h1 { "Minimal Rust Full-Stack Counter" }
            p { class: "subtitle", "Dioxus UI + Rust backend + SQLite persistence" }
            p { class: "value-line", "Current value: " span { id: "current-value", "{value}" } }
            button { id: "increment-button", class: "btn", "Increment value" }
        }
    }
}

#[component]
fn SettingsPage() -> Element {
    rsx! {
        section { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Sample settings route" }
        }
    }
}

pub fn render_document(route: Route, value: i64) -> String {
    let body = dioxus_ssr::render_element(rsx! {
        App { route: route.clone(), value }
    });

    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"></head><body>{body}</body></html>"
    )
}

fn styles() -> &'static str {
    r#"
:root {
  color-scheme: dark;
}
body {
  margin: 0;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Helvetica, Arial, sans-serif;
  background: radial-gradient(circle at top, #1f2937 0%, #0b1020 50%, #070b16 100%);
  min-height: 100vh;
  color: #e5e7eb;
}
.app-shell {
  max-width: 760px;
  margin: 0 auto;
  padding: 2rem 1rem;
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
.top-nav a:hover {
  background: rgba(51, 65, 85, 0.9);
}
.card {
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.3);
  border-radius: 14px;
  padding: 1.5rem;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
}
.subtitle {
  color: #94a3b8;
}
.value-line {
  font-size: 1.25rem;
}
.btn {
  margin-top: 0.75rem;
  border: 0;
  border-radius: 10px;
  background: linear-gradient(135deg, #22d3ee, #3b82f6);
  color: #031525;
  font-weight: 600;
  padding: 0.7rem 1rem;
  cursor: pointer;
}
.btn:hover {
  filter: brightness(1.08);
}
"#
}

fn interaction_script() -> &'static str {
    r#"
async function syncValue() {
  const response = await fetch('/api/value');
  const payload = await response.json();
  const target = document.getElementById('current-value');
  if (target) {
    target.textContent = payload.value;
  }
}

async function incrementValue() {
  const response = await fetch('/api/value/increment', { method: 'POST' });
  const payload = await response.json();
  const target = document.getElementById('current-value');
  if (target) {
    target.textContent = payload.value;
  }
}

window.addEventListener('DOMContentLoaded', () => {
  const button = document.getElementById('increment-button');
  if (button) {
    button.addEventListener('click', incrementValue);
  }
  syncValue();
});
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_landing_content() {
        let html = render_document(Route::Landing {}, 7);
        assert!(html.contains("Minimal Rust Full-Stack Counter"));
        assert!(html.contains("id=\"current-value\""));
        assert!(html.contains(">7<"));
    }

    #[test]
    fn renders_settings_content() {
        let html = render_document(Route::Settings {}, 0);
        assert!(html.contains("Sample settings route"));
    }

    #[test]
    fn uses_response_value_for_next_value() {
        assert_eq!(next_value_after_increment(41, 42), 42);
    }

    #[test]
    fn does_not_decrease_displayed_value() {
        assert_eq!(next_value_after_increment(42, 41), 42);
    }
}
