//! Thin SSR shim that delegates rendering to the shared `omnibus-frontend`
//! crate. This layer goes away in Commit 4 when `dioxus-fullstack` takes
//! over server rendering + WASM hydration end-to-end.

use dioxus::prelude::*;

pub use omnibus_frontend::Route;

pub fn render_document(route: Route) -> String {
    let body = dioxus_ssr::render_element(rsx! {
        style { {omnibus_frontend::STYLES} }
        div { class: "app-shell",
            omnibus_frontend::Nav {}
            main {
                match route {
                    Route::Landing {} => rsx! { omnibus_frontend::LandingPage {} },
                    Route::Settings {} => rsx! { omnibus_frontend::SettingsPage {} },
                }
            }
        }
    });

    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Omnibus</title></head><body>{body}</body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_landing_content() {
        let html = render_document(Route::Landing {});
        assert!(html.contains("Omnibus Counter"));
        assert!(html.contains("data-testid=\"current-value\""));
        assert!(html.contains("id=\"increment-button\""));
    }

    #[test]
    fn renders_settings_content() {
        let html = render_document(Route::Settings {});
        assert!(html.contains("id=\"settings-form\""));
        assert!(html.contains("id=\"ebook-library-path\""));
        assert!(html.contains("id=\"audiobook-library-path\""));
        assert!(html.contains("data-testid=\"ebook-library-contents\""));
        assert!(html.contains("data-testid=\"audiobook-library-contents\""));
    }
}
