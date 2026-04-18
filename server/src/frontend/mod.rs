pub mod components;
pub mod pages;

use dioxus::prelude::*;
use dioxus_router::Routable;

use components::TopNav;
use pages::{LandingPage, SettingsPage};

use crate::db::Settings;

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
    pub settings: Settings,
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
            TopNav {}
            main {
                match props.route {
                    Route::Landing {} => rsx! { LandingPage { value: Some(props.value) } },
                    Route::Settings {} => rsx! { SettingsPage { settings: Some(props.settings.clone()) } },
                }
            }
        }
        script { dangerous_inner_html: "{interaction_script()}" }
    }
}

pub fn next_value_after_increment(current: i64, response_value: i64) -> i64 {
    if response_value < current {
        current
    } else {
        response_value
    }
}

pub fn render_document(route: Route, value: i64, settings: Settings) -> String {
    let body = dioxus_ssr::render_element(rsx! {
        App { route: route.clone(), value, settings }
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
.settings-form {
  display: flex;
  flex-direction: column;
  gap: 1.25rem;
  margin-top: 1.25rem;
}
.settings-field {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}
.settings-field label {
  font-size: 0.875rem;
  font-weight: 500;
  color: #cbd5e1;
}
.settings-field input[type="text"] {
  background: rgba(30, 41, 59, 0.8);
  border: 1px solid rgba(100, 116, 139, 0.4);
  border-radius: 8px;
  color: #e5e7eb;
  font-size: 0.95rem;
  padding: 0.55rem 0.75rem;
  width: 100%;
  box-sizing: border-box;
}
.settings-field input[type="text"]:focus {
  outline: none;
  border-color: #3b82f6;
}
.settings-status {
  font-size: 0.875rem;
  margin-top: 0.5rem;
  min-height: 1.2em;
}
.settings-status.success { color: #34d399; }
.settings-status.error   { color: #f87171; }
.library-card { margin-top: 1.25rem; }
.library-card h2 { font-size: 1rem; font-weight: 600; margin-bottom: 0.75rem; color: #cbd5e1; }
.library-path { font-size: 0.8rem; color: #64748b; margin-bottom: 0.4rem; font-family: monospace; }
.library-count { font-size: 0.85rem; color: #94a3b8; margin-bottom: 0.5rem; }
.library-loading { color: #64748b; font-size: 0.875rem; }
.library-empty { color: #64748b; font-size: 0.875rem; }
.library-error { color: #f87171; font-size: 0.875rem; }
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
.library-file-list li {
  font-size: 0.875rem;
  font-family: monospace;
  padding: 0.3rem 0.5rem;
  background: rgba(30, 41, 59, 0.5);
  border-radius: 6px;
  color: #e2e8f0;
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

async function saveSettings(event) {
  event.preventDefault();
  const status = document.getElementById('settings-status');
  const ebookVal = document.getElementById('ebook-library-path').value.trim();
  const audiobookVal = document.getElementById('audiobook-library-path').value.trim();
  try {
    const response = await fetch('/api/settings', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        ebook_library_path: ebookVal || null,
        audiobook_library_path: audiobookVal || null
      })
    });
    if (response.ok) {
      status.textContent = 'Settings saved.';
      status.className = 'settings-status success';
      loadLibrary();
    } else {
      status.textContent = 'Failed to save settings.';
      status.className = 'settings-status error';
    }
  } catch {
    status.textContent = 'Network error.';
    status.className = 'settings-status error';
  }
}

function renderLibrarySection(containerId, section) {
  const container = document.getElementById(containerId);
  if (!container) return;
  if (!section.path) {
    container.innerHTML = '<p class="library-empty">No path configured.</p>';
    return;
  }
  if (section.error) {
    container.innerHTML = '<p class="library-error">\u26a0 ' + section.error + '</p>';
    return;
  }
  if (section.files.length === 0) {
    container.innerHTML = '<p class="library-empty">No files found in <code>' + section.path + '</code></p>';
    return;
  }
  const items = section.files.map(function(f) { return '<li>' + f + '</li>'; }).join('');
  container.innerHTML =
    '<p class="library-path">' + section.path + '</p>' +
    '<p class="library-count">' + section.files.length + ' file(s)</p>' +
    '<ul class="library-file-list">' + items + '</ul>';
}

async function loadLibrary() {
  const ebookEl = document.getElementById('ebook-library-contents');
  const audiobookEl = document.getElementById('audiobook-library-contents');
  if (!ebookEl && !audiobookEl) return;
  try {
    const response = await fetch('/api/library');
    const data = await response.json();
    renderLibrarySection('ebook-library-contents', data.ebooks);
    renderLibrarySection('audiobook-library-contents', data.audiobooks);
  } catch (e) {
    if (ebookEl) ebookEl.innerHTML = '<p class="library-error">Failed to load library.</p>';
    if (audiobookEl) audiobookEl.innerHTML = '<p class="library-error">Failed to load library.</p>';
  }
}

window.addEventListener('DOMContentLoaded', () => {
  const button = document.getElementById('increment-button');
  if (button) {
    button.addEventListener('click', incrementValue);
  }
  const settingsForm = document.getElementById('settings-form');
  if (settingsForm) {
    settingsForm.addEventListener('submit', saveSettings);
  }
  syncValue();
  loadLibrary();
});
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_landing_content() {
        let html = render_document(Route::Landing {}, 7, Settings::default());
        assert!(html.contains("Minimal Rust Full-Stack Counter"));
        assert!(html.contains("id=\"current-value\""));
        assert!(html.contains("data-testid=\"current-value\""));
        assert!(html.contains(">7<"));
    }

    #[test]
    fn renders_settings_form_with_empty_inputs_by_default() {
        let html = render_document(Route::Settings {}, 0, Settings::default());
        assert!(html.contains("id=\"settings-form\""));
        assert!(html.contains("id=\"ebook-library-path\""));
        assert!(html.contains("id=\"audiobook-library-path\""));
        assert!(html.contains("id=\"ebook-library-contents\""));
        assert!(html.contains("id=\"audiobook-library-contents\""));
    }

    #[test]
    fn renders_settings_form_with_populated_values() {
        let settings = Settings {
            ebook_library_path: Some("/srv/ebooks".to_string()),
            audiobook_library_path: Some("/srv/audio".to_string()),
        };
        let html = render_document(Route::Settings {}, 0, settings);
        assert!(html.contains("/srv/ebooks"));
        assert!(html.contains("/srv/audio"));
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
