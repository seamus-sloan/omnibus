use dioxus::prelude::*;

use crate::{data, use_server_url};

/// Counter placeholder — fetches the current value on mount, increments it on click.
///
/// Uses reactive signals so the same component drives both the web (hydrated)
/// and mobile (Dioxus Native) targets. IDs and testids are preserved for
/// Playwright compatibility.
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    let mut value = use_signal(|| 0i64);
    let mut error = use_signal(|| None::<String>);

    let url_for_fetch = server_url.clone();
    use_effect(move || {
        let url = url_for_fetch.clone();
        spawn(async move {
            match data::get_value(&url).await {
                Ok(v) => {
                    value.set(v);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    rsx! {
        section { class: "card",
            h1 { "Omnibus Counter" }
            p { class: "subtitle", "Dioxus UI + Rust backend + SQLite persistence" }
            if let Some(msg) = error() {
                p { class: "error", "⚠ {msg}" }
            }
            p { class: "value-line",
                "Current value: "
                span {
                    id: "current-value",
                    "data-testid": "current-value",
                    "{value}"
                }
            }
            button {
                id: "increment-button",
                class: "btn",
                onclick: move |_| {
                    let url = server_url.clone();
                    spawn(async move {
                        match data::post_increment(&url).await {
                            Ok(v) => {
                                value.set(v);
                                error.set(None);
                            }
                            Err(e) => error.set(Some(e)),
                        }
                    });
                },
                "Increment value"
            }
        }
    }
}
