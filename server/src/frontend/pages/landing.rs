use dioxus::prelude::*;

#[component]
pub fn LandingPage(value: Option<i64>) -> Element {
    let value = value.unwrap_or_default();
    rsx! {
        section { class: "card",
            h1 { "Minimal Rust Full-Stack Counter" }
            p { class: "subtitle", "Dioxus UI + Rust backend + SQLite persistence" }
            p { class: "value-line", "Current value: " span { id: "current-value", "data-testid": "current-value",
                "{value}"
            } }
            button { id: "increment-button", class: "btn", "Increment value" }
        }
    }
}
