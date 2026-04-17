use dioxus::prelude::*;

#[component]
pub fn SettingsPage() -> Element {
    rsx! {
        section { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Sample settings route" }
        }
    }
}
