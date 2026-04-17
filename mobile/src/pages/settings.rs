use dioxus::prelude::*;

use crate::ServerUrl;

#[component]
pub fn SettingsPage() -> Element {
    let server_url = use_context::<ServerUrl>();

    rsx! {
        div { class: "card",
            h1 { "Settings" }
            p { class: "subtitle", "Connected to: {server_url.0}" }
        }
    }
}
